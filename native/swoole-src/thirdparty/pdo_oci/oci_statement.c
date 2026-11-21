/*
  +----------------------------------------------------------------------+
  | Copyright (c) The PHP Group                                          |
  +----------------------------------------------------------------------+
  | This source file is subject to version 3.01 of the PHP license,      |
  | that is bundled with this package in the file LICENSE, and is        |
  | available through the world-wide-web at the following url:           |
  | https://www.php.net/license/3_01.txt                                 |
  | If you did not receive a copy of the PHP license and are unable to   |
  | obtain it through the world-wide-web, please send a note to          |
  | license@php.net so we can mail you a copy immediately.               |
  +----------------------------------------------------------------------+
  | Author: Wez Furlong <wez@php.net>                                    |
  +----------------------------------------------------------------------+
*/

#include "php_swoole_oracle.h"
#include "php.h"
#include "php_ini.h"
#include "ext/standard/info.h"
#include "pdo/php_pdo.h"
#include "Zend/zend_extensions.h"

#define PDO_OCI_LOBMAXSIZE (4294967295UL) /* OCI_LOBMAXSIZE */

#define STMT_CALL(name, params)                                                                                        \
    do {                                                                                                               \
        S->last_err = name params;                                                                                     \
        S->last_err = _oci_error(S->err, stmt->dbh, stmt, #name, S->last_err, FALSE, __FILE__, __LINE__);              \
        if (S->last_err) {                                                                                             \
            return 0;                                                                                                  \
        }                                                                                                              \
    } while (0)

#define STMT_CALL_MSG(name, msg, params)                                                                               \
    do {                                                                                                               \
        S->last_err = name params;                                                                                     \
        S->last_err = _oci_error(S->err, stmt->dbh, stmt, #name ": " #msg, S->last_err, FALSE, __FILE__, __LINE__);    \
        if (S->last_err) {                                                                                             \
            return 0;                                                                                                  \
        }                                                                                                              \
    } while (0)

static php_stream *oci_create_lob_stream(pdo_stmt_t *stmt, OCILobLocator *lob);

#define OCI_TEMPLOB_CLOSE(envhp, svchp, errhp, lob)                                                                    \
    do {                                                                                                               \
        boolean isTempLOB;                                                                                             \
        OCILobIsTemporary(envhp, errhp, lob, &isTempLOB);                                                              \
        if (isTempLOB) OCILobFreeTemporary(svchp, errhp, lob);                                                         \
    } while (0)

static int oci_stmt_dtor(pdo_stmt_t *stmt) /* {{{ */
{
    pdo_oci_stmt *S = (pdo_oci_stmt *) stmt->driver_data;
    HashTable *BC = stmt->bound_columns;
    HashTable *BP = stmt->bound_params;

    int i;

    if (S->stmt) {
        /* cancel server side resources for the statement if we didn't
         * fetch it all */
        OCIStmtFetch(S->stmt, S->err, 0, OCI_FETCH_NEXT, OCI_DEFAULT);

        /* free the handle */
        OCIHandleFree(S->stmt, OCI_HTYPE_STMT);
        S->stmt = NULL;
    }
    if (S->err) {
        OCIHandleFree(S->err, OCI_HTYPE_ERROR);
        S->err = NULL;
    }

    /* need to ensure these go away now */
    if (BC) {
        zend_hash_destroy(BC);
        FREE_HASHTABLE(stmt->bound_columns);
        stmt->bound_columns = NULL;
    }

    if (BP) {
        zend_hash_destroy(BP);
        FREE_HASHTABLE(stmt->bound_params);
        stmt->bound_params = NULL;
    }

    swr_oci_einfo_free(stmt->dbh, stmt);

#if PHP_API_VERSION < 20240925
    bool server_obj_usable =
        !Z_ISUNDEF(stmt->database_object_handle) &&
        IS_OBJ_VALID(EG(objects_store).object_buckets[Z_OBJ_HANDLE(stmt->database_object_handle)]) &&
        !(OBJ_FLAGS(Z_OBJ(stmt->database_object_handle)) & IS_OBJ_FREE_CALLED);
#else
    bool server_obj_usable = php_pdo_stmt_valid_db_obj_handle(stmt);
#endif

    if (S->cols) {
        for (i = 0; i < stmt->column_count; i++) {
            if (S->cols[i].data) {
                switch (S->cols[i].dtype) {
                case SQLT_BLOB:
                case SQLT_CLOB:
                    if (server_obj_usable) {
                        OCI_TEMPLOB_CLOSE(swr_oci_env(S->dbh), swr_oci_svc(S->dbh), swr_oci_err(S->dbh), (OCILobLocator *) S->cols[i].data);
                    }
                    OCIDescriptorFree(S->cols[i].data, OCI_DTYPE_LOB);
                    break;
                default:
                    efree(S->cols[i].data);
                }
            }
        }
        efree(S->cols);
        S->cols = NULL;
    }
    efree(S);

    stmt->driver_data = NULL;

    return 1;
} /* }}} */

static int oci_stmt_execute(pdo_stmt_t *stmt) /* {{{ */
{
    pdo_oci_stmt *S = (pdo_oci_stmt *) stmt->driver_data;
    ub4 rowcount;
    b4 mode;

    if (!S->stmt_type) {
        STMT_CALL_MSG(
            OCIAttrGet, "OCI_ATTR_STMT_TYPE", (S->stmt, OCI_HTYPE_STMT, &S->stmt_type, 0, OCI_ATTR_STMT_TYPE, S->err));
    }

    if (stmt->executed) {
        /* ensure that we cancel the cursor from a previous fetch */
        OCIStmtFetch(S->stmt, S->err, 0, OCI_FETCH_NEXT, OCI_DEFAULT);
    }

#ifdef OCI_STMT_SCROLLABLE_READONLY /* needed for oci8 ? */
    if (S->exec_type == OCI_STMT_SCROLLABLE_READONLY) {
        mode = OCI_STMT_SCROLLABLE_READONLY;
    } else
#endif
        if (stmt->dbh->auto_commit && !stmt->dbh->in_txn) {
        mode = OCI_COMMIT_ON_SUCCESS;
    } else {
        mode = OCI_DEFAULT;
    }

    STMT_CALL(
        OCIStmtExecute,
        (swr_oci_svc(S->dbh), S->stmt, S->err, (S->stmt_type == OCI_STMT_SELECT && !S->have_blobs) ? 0 : 1, 0, NULL, NULL, mode));

    if (!stmt->executed) {
        ub4 colcount;
        /* do first-time-only definition of bind/mapping stuff */

        /* how many columns do we have ? */
        STMT_CALL_MSG(
            OCIAttrGet, "ATTR_PARAM_COUNT", (S->stmt, OCI_HTYPE_STMT, &colcount, 0, OCI_ATTR_PARAM_COUNT, S->err));

        stmt->column_count = (int) colcount;

        if (S->cols) {
            int i;
            for (i = 0; i < stmt->column_count; i++) {
                if (S->cols[i].data) {
                    switch (S->cols[i].dtype) {
                    case SQLT_BLOB:
                    case SQLT_CLOB:
                        /* do nothing */
                        break;
                    default:
                        efree(S->cols[i].data);
                    }
                }
            }
            efree(S->cols);
        }

        S->cols = ecalloc(colcount, sizeof(pdo_oci_column));
    }

    STMT_CALL_MSG(OCIAttrGet, "ATTR_ROW_COUNT", (S->stmt, OCI_HTYPE_STMT, &rowcount, 0, OCI_ATTR_ROW_COUNT, S->err));
    stmt->row_count = (long) rowcount;

    return 1;
} /* }}} */

static sb4 oci_bind_input_cb(
    dvoid *ctx, OCIBind *bindp, ub4 iter, ub4 index, dvoid **bufpp, ub4 *alenp, ub1 *piecep, dvoid **indpp) /* {{{ */
{
    struct pdo_bound_param_data *param = (struct pdo_bound_param_data *) ctx;
    pdo_oci_bound_param *P = (pdo_oci_bound_param *) param->driver_data;
    zval *parameter;

    ZEND_ASSERT(param);

    *indpp = &P->indicator;

    // TODO Duplicate dereferencing operation
    if (Z_ISREF(param->parameter))
        parameter = Z_REFVAL(param->parameter);
    else
        parameter = &param->parameter;

    if (P->thing) {
        *bufpp = P->thing;
        *alenp = sizeof(void *);
    } else if (ZVAL_IS_NULL(parameter)) {
        /* insert a NULL value into the column */
        P->indicator = -1; /* NULL */
        *bufpp = 0;
        *alenp = -1;
    } else if (!P->thing) {
        /* regular string bind */
    	// safe: use preconverted C buffer (no Zend API here `try_convert_to_string`)
        if (!P->preconv_buf) {
            return OCI_ERROR;
        }
        *bufpp = P->preconv_buf;
        *alenp = P->preconv_len;
    }

    *piecep = OCI_ONE_PIECE;
    return OCI_CONTINUE;
} /* }}} */

/**
 * This function will be called during the execution of OCIStmtExecute,
 * while the OCIStmtExecute function is executed in an AIO asynchronous thread within asynchronous IO.
 *
 */
static sb4 oci_bind_output_cb(dvoid *ctx,
                              OCIBind *bindp,
                              ub4 iter,
                              ub4 index,
                              dvoid **bufpp,
                              ub4 **alenpp,
                              ub1 *piecep,
                              dvoid **indpp,
                              ub2 **rcodepp) /* {{{ */
{
    struct pdo_bound_param_data *param = (struct pdo_bound_param_data *) ctx;
    pdo_oci_bound_param *P = (pdo_oci_bound_param *) param->driver_data;
    zval *parameter;

    ZEND_ASSERT(param);

    if (Z_ISREF(param->parameter)) {
        parameter = Z_REFVAL(param->parameter);
    } else {
        parameter = &param->parameter;
    }

    if (PDO_PARAM_TYPE(param->param_type) == PDO_PARAM_LOB) {
        P->actual_len = sizeof(OCILobLocator *);
        *bufpp = P->thing;
        *alenpp = &P->actual_len;
        *piecep = OCI_ONE_PIECE;
        *rcodepp = &P->retcode;
        *indpp = &P->indicator;
        return OCI_CONTINUE;
    }

    if (Z_TYPE_P(parameter) == IS_OBJECT || Z_TYPE_P(parameter) == IS_RESOURCE) {
        return OCI_CONTINUE;
    }

    // It can be safely released in an aio thread, this is a persistent string,
    // allocated with malloc instead of ZendMM emalloc.
    zval_ptr_dtor(parameter);

    // Must use malloc for persistent strings
    Z_STR_P(parameter) = zend_string_alloc(param->max_value_len, 1);
    P->used_for_output = 1;

    P->actual_len = (ub4) Z_STRLEN_P(parameter);
    *alenpp = &P->actual_len;
    *bufpp = (Z_STR_P(parameter))->val;
    *piecep = OCI_ONE_PIECE;
    *rcodepp = &P->retcode;
    *indpp = &P->indicator;

    return OCI_CONTINUE;
} /* }}} */

static int oci_stmt_param_hook(pdo_stmt_t *stmt,
                               struct pdo_bound_param_data *param,
                               enum pdo_param_event event_type) /* {{{ */
{
    pdo_oci_stmt *S = (pdo_oci_stmt *) stmt->driver_data;

    /* we're only interested in parameters for prepared SQL right now */
    if (param->is_param) {
        pdo_oci_bound_param *P;
        sb4 value_sz = -1;
        zval *parameter;

        if (Z_ISREF(param->parameter))
            parameter = Z_REFVAL(param->parameter);
        else
            parameter = &param->parameter;

        P = (pdo_oci_bound_param *) param->driver_data;

        switch (event_type) {
        case PDO_PARAM_EVT_FETCH_PRE:
        case PDO_PARAM_EVT_FETCH_POST:
        case PDO_PARAM_EVT_NORMALIZE:
            /* Do nothing */
            break;

        case PDO_PARAM_EVT_FREE:
            P = param->driver_data;
            if (P && P->preconv_buf) {
				efree(P->preconv_buf);
				P->preconv_buf = NULL;
				P->preconv_len = 0;
			}
            if (P && P->thing) {
                OCI_TEMPLOB_CLOSE(swr_oci_env(S->dbh), swr_oci_svc(S->dbh), swr_oci_err(S->dbh), P->thing);
                OCIDescriptorFree(P->thing, OCI_DTYPE_LOB);
                P->thing = NULL;
                efree(P);
            } else if (P) {
                efree(P);
            }
            break;

        case PDO_PARAM_EVT_ALLOC:
            P = (pdo_oci_bound_param *) ecalloc(1, sizeof(pdo_oci_bound_param));
            param->driver_data = P;

            /* figure out what we're doing */
            switch (PDO_PARAM_TYPE(param->param_type)) {
            case PDO_PARAM_STMT:
                return 0;

            case PDO_PARAM_LOB:
                /* P->thing is now an OCILobLocator * */
                P->oci_type = SQLT_BLOB;
                value_sz = (sb4) sizeof(OCILobLocator *);
                break;

            case PDO_PARAM_STR:
            default:
                P->oci_type = SQLT_CHR;
                value_sz = (sb4) param->max_value_len;
                if (param->max_value_len == 0) {
                    value_sz = (sb4) 1332; /* maximum size before value is interpreted as a LONG value */
                }
            }

            if (param->name) {
                STMT_CALL(OCIBindByName,
                          (S->stmt,
                           &P->bind,
                           S->err,
                           (text *) param->name->val,
                           (sb4) param->name->len,
                           0,
                           value_sz,
                           P->oci_type,
                           &P->indicator,
                           0,
                           &P->retcode,
                           0,
                           0,
                           OCI_DATA_AT_EXEC));
            } else {
                STMT_CALL(OCIBindByPos,
                          (S->stmt,
                           &P->bind,
                           S->err,
                           ((ub4) param->paramno) + 1,
                           0,
                           value_sz,
                           P->oci_type,
                           &P->indicator,
                           0,
                           &P->retcode,
                           0,
                           0,
                           OCI_DATA_AT_EXEC));
            }


            STMT_CALL(OCIBindDynamic, (P->bind, S->err, param, oci_bind_input_cb, param, oci_bind_output_cb));

            return 1;

        case PDO_PARAM_EVT_EXEC_PRE:
            P->indicator = 0;
            P->used_for_output = 0;
            if (PDO_PARAM_TYPE(param->param_type) == PDO_PARAM_LOB) {
                ub4 empty = 0;
                STMT_CALL(OCIDescriptorAlloc, (swr_oci_env(S->dbh), &P->thing, OCI_DTYPE_LOB, 0, NULL));
                STMT_CALL(OCIAttrSet, (P->thing, OCI_DTYPE_LOB, &empty, 0, OCI_ATTR_LOBEMPTY, S->err));
                S->have_blobs = 1;
            }

            /* in PDO_PARAM_EVT_EXEC_PRE, executed in PHP-context (main thread) */
            if (PDO_PARAM_TYPE(param->param_type) != PDO_PARAM_LOB && Z_TYPE_P(parameter) != IS_NULL) {
                convert_to_string(parameter);
                /* allocate persistent or temporary buffer that outlives dispatch to async thread */
                P->preconv_len = Z_STRLEN_P(parameter);
                P->preconv_buf = emalloc(P->preconv_len + 1);
                memcpy(P->preconv_buf, Z_STRVAL_P(parameter), P->preconv_len);
                P->preconv_buf[P->preconv_len] = '\0';
            }

            return 1;

        case PDO_PARAM_EVT_EXEC_POST:
            /* fixup stuff set in motion in oci_bind_output_cb */
            if (P->used_for_output) {
                if (P->indicator == -1) {
                    /* set up a NULL value */
                    if (Z_TYPE_P(parameter) == IS_STRING) {
                        /* OCI likes to stick non-terminated strings in things */
                        *Z_STRVAL_P(parameter) = '\0';
                    }
                    zval_ptr_dtor_str(parameter);
                    ZVAL_UNDEF(parameter);
                } else if (Z_TYPE_P(parameter) == IS_STRING) {
                    Z_STR_P(parameter) = zend_string_init(Z_STRVAL_P(parameter), P->actual_len, 1);
                }
            } else if (PDO_PARAM_TYPE(param->param_type) == PDO_PARAM_LOB && P->thing) {
                php_stream *stm;

                if (Z_TYPE_P(parameter) == IS_NULL) {
                    /* if the param is NULL, then we assume that they
                     * wanted to bind a lob locator into it from the query
                     * */

                    stm = oci_create_lob_stream(stmt, (OCILobLocator *) P->thing);
                    if (stm) {
                        OCILobOpen(swr_oci_svc(S->dbh), S->err, (OCILobLocator *) P->thing, OCI_LOB_READWRITE);
                        php_stream_to_zval(stm, parameter);
                    }
                } else {
                    /* we're a LOB being used for insert; transfer the data now */
                    size_t n;
                    ub4 amt, offset = 1;
                    char *consume;

                    php_stream_from_zval_no_verify(stm, parameter);
                    if (stm) {
                        OCILobOpen(swr_oci_svc(S->dbh), S->err, (OCILobLocator *) P->thing, OCI_LOB_READWRITE);
                        do {
                            char buf[8192];
                            n = php_stream_read(stm, buf, sizeof(buf));
                            if ((int) n <= 0) {
                                break;
                            }
                            consume = buf;
                            do {
                                amt = (ub4) n;
                                OCILobWrite(swr_oci_svc(S->dbh),
                                            S->err,
                                            (OCILobLocator *) P->thing,
                                            &amt,
                                            offset,
                                            consume,
                                            (ub4) n,
                                            OCI_ONE_PIECE,
                                            NULL,
                                            NULL,
                                            0,
                                            SQLCS_IMPLICIT);
                                offset += amt;
                                n -= amt;
                                consume += amt;
                            } while (n);
                        } while (1);
                        OCILobClose(swr_oci_svc(S->dbh), S->err, (OCILobLocator *) P->thing);
                        OCILobFlushBuffer(swr_oci_svc(S->dbh), S->err, (OCILobLocator *) P->thing, 0);
                    } else if (Z_TYPE_P(parameter) == IS_STRING) {
                        /* stick the string into the LOB */
                        consume = Z_STRVAL_P(parameter);
                        n = Z_STRLEN_P(parameter);
                        if (n) {
                            OCILobOpen(swr_oci_svc(S->dbh), S->err, (OCILobLocator *) P->thing, OCI_LOB_READWRITE);
                            while (n) {
                                amt = (ub4) n;
                                OCILobWrite(swr_oci_svc(S->dbh),
                                            S->err,
                                            (OCILobLocator *) P->thing,
                                            &amt,
                                            offset,
                                            consume,
                                            (ub4) n,
                                            OCI_ONE_PIECE,
                                            NULL,
                                            NULL,
                                            0,
                                            SQLCS_IMPLICIT);
                                consume += amt;
                                n -= amt;
                            }
                            OCILobClose(swr_oci_svc(S->dbh), S->err, (OCILobLocator *) P->thing);
                        }
                    }
                    OCI_TEMPLOB_CLOSE(swr_oci_env(S->dbh), swr_oci_svc(S->dbh), swr_oci_err(S->dbh), P->thing);
                    OCIDescriptorFree(P->thing, OCI_DTYPE_LOB);
                    P->thing = NULL;
                }
            }

            if (P->preconv_buf) {
				efree(P->preconv_buf);
				P->preconv_buf = NULL;
				P->preconv_len = 0;
			}

            return 1;
        }
    }

    return 1;
} /* }}} */

/* Rust fetches the row (crates/oci/src/statement.rs). PDO's method table takes an int. */
extern bool swr_oci_fetch(pdo_stmt_t *stmt, pdo_dbh_t *dbh, uint32_t orientation, zend_long offset);

static int oci_stmt_fetch(pdo_stmt_t *stmt, enum pdo_fetch_orientation ori, zend_long offset) /* {{{ */
{
    return swr_oci_fetch(stmt, stmt->dbh, (uint32_t) ori, offset) ? 1 : 0;
} /* }}} */

/**
 * This function is thread-safe as it does not call any ZendAPI.
 */
static sb4 oci_define_callback(
    dvoid *octxp, OCIDefine *define, ub4 iter, dvoid **bufpp, ub4 **alenpp, ub1 *piecep, dvoid **indpp, ub2 **rcodepp) {
    pdo_oci_column *col = (pdo_oci_column *) octxp;

    switch (col->dtype) {
    case SQLT_BLOB:
    case SQLT_CLOB:
        *piecep = OCI_ONE_PIECE;
        *bufpp = col->data;
        *alenpp = &col->datalen;
        *indpp = (dvoid *) &col->indicator;
        break;
        EMPTY_SWITCH_DEFAULT_CASE();
    }

    return OCI_CONTINUE;
}

static int oci_stmt_describe(pdo_stmt_t *stmt, int colno) /* {{{ */
{
    pdo_oci_stmt *S = (pdo_oci_stmt *) stmt->driver_data;
    OCIParam *param = NULL;
    text *colname;
    ub2 dtype, data_size, precis;
    ub4 namelen;
    struct pdo_column_data *col = &stmt->columns[colno];
    bool dyn = FALSE;

    /* describe the column */
    STMT_CALL(OCIParamGet, (S->stmt, OCI_HTYPE_STMT, S->err, (dvoid *) &param, colno + 1));

    /* what type ? */
    STMT_CALL_MSG(OCIAttrGet, "OCI_ATTR_DATA_TYPE", (param, OCI_DTYPE_PARAM, &dtype, 0, OCI_ATTR_DATA_TYPE, S->err));

    /* how big ? */
    STMT_CALL_MSG(
        OCIAttrGet, "OCI_ATTR_DATA_SIZE", (param, OCI_DTYPE_PARAM, &data_size, 0, OCI_ATTR_DATA_SIZE, S->err));

    /* precision ? */
    STMT_CALL_MSG(OCIAttrGet, "OCI_ATTR_PRECISION", (param, OCI_DTYPE_PARAM, &precis, 0, OCI_ATTR_PRECISION, S->err));

    /* name ? */
    STMT_CALL_MSG(OCIAttrGet, "OCI_ATTR_NAME", (param, OCI_DTYPE_PARAM, &colname, &namelen, OCI_ATTR_NAME, S->err));

    col->precision = precis;
    col->maxlen = data_size;
    col->name = zend_string_init((char *) colname, namelen, 0);

    S->cols[colno].dtype = dtype;

    /* how much room do we need to store the field */
    switch (dtype) {
    case SQLT_LBI:
    case SQLT_LNG:
        if (dtype == SQLT_LBI) {
            dtype = SQLT_BIN;
        } else {
            dtype = SQLT_CHR;
        }
        S->cols[colno].datalen = 512; /* XXX should be INT_MAX and fetched by pieces */
        S->cols[colno].data = emalloc(S->cols[colno].datalen + 1);
        break;

    case SQLT_BLOB:
    case SQLT_CLOB:
        STMT_CALL(OCIDescriptorAlloc, (swr_oci_env(S->dbh), (dvoid **) &S->cols[colno].data, OCI_DTYPE_LOB, 0, NULL));
        S->cols[colno].datalen = sizeof(OCILobLocator *);
        dyn = TRUE;
        break;

    case SQLT_BIN:
    default:
        if (dtype == SQLT_DAT || dtype == SQLT_NUM || dtype == SQLT_RDD
#ifdef SQLT_TIMESTAMP
            || dtype == SQLT_TIMESTAMP
#endif
#ifdef SQLT_TIMESTAMP_TZ
            || dtype == SQLT_TIMESTAMP_TZ
#endif
        ) {
            /* should be big enough for most date formats and numbers */
            S->cols[colno].datalen = 512;
#if defined(SQLT_IBFLOAT) && defined(SQLT_IBDOUBLE)
        } else if (dtype == SQLT_IBFLOAT || dtype == SQLT_IBDOUBLE) {
            S->cols[colno].datalen = 1024;
#endif
        } else if (dtype == SQLT_BIN) {
            S->cols[colno].datalen = (ub4) col->maxlen * 2; /* raw characters to hex digits */
        } else {
            S->cols[colno].datalen = (ub4) (col->maxlen * swr_oci_max_char_width(S->dbh));
        }

        S->cols[colno].data = emalloc(S->cols[colno].datalen + 1);
        dtype = SQLT_CHR;
    }

    STMT_CALL(OCIDefineByPos,
              (S->stmt,
               &S->cols[colno].def,
               S->err,
               colno + 1,
               S->cols[colno].data,
               S->cols[colno].datalen,
               dtype,
               &S->cols[colno].indicator,
               &S->cols[colno].fetched_len,
               &S->cols[colno].retcode,
               dyn ? OCI_DYNAMIC_FETCH : OCI_DEFAULT));

    if (dyn) {
        STMT_CALL(OCIDefineDynamic, (S->cols[colno].def, S->err, &S->cols[colno], oci_define_callback));
    }

    return 1;
} /* }}} */

struct _oci_lob_env {
    OCISvcCtx *svc;
    OCIError *err;
};
typedef struct _oci_lob_env oci_lob_env;

struct oci_lob_self {
    zval dbh;
    pdo_stmt_t *stmt;
    pdo_oci_stmt *S;
    OCILobLocator *lob;
    oci_lob_env *E;
    ub4 offset;
    ub1 csfrm;
};

static ssize_t oci_blob_write(php_stream *stream, const char *buf, size_t count) {
    struct oci_lob_self *self = (struct oci_lob_self *) stream->abstract;
    ub4 amt;
    sword r;

    amt = (ub4) count;
    r = OCILobWrite(self->E->svc,
                    self->E->err,
                    self->lob,
                    &amt,
                    self->offset,
                    (char *) buf,
                    (ub4) count,
                    OCI_ONE_PIECE,
                    NULL,
                    NULL,
                    0,
                    SQLCS_IMPLICIT);

    if (r != OCI_SUCCESS) {
        return (ssize_t) -1;
    }

    self->offset += amt;
    return amt;
}

static ssize_t oci_blob_read(php_stream *stream, char *buf, size_t count) {
    struct oci_lob_self *self = (struct oci_lob_self *) stream->abstract;
#if HAVE_OCILOBREAD2
    oraub8 byte_amt = (oraub8) count;
    oraub8 char_amt = 0;

    sword r = OCILobRead2(self->E->svc,
                          self->E->err,
                          self->lob,
                          &byte_amt,
                          &char_amt,
                          (oraub8) self->offset,
                          buf,
                          (oraub8) count,
                          OCI_ONE_PIECE,
                          NULL,
                          NULL,
                          0,
                          self->csfrm);
#else
    ub4 byte_amt = (ub4) count;

    sword r = OCILobRead(self->E->svc,
                         self->E->err,
                         self->lob,
                         &byte_amt,
                         self->offset,
                         buf,
                         (ub4) count,
                         NULL,
                         NULL,
                         0,
                         SQLCS_IMPLICIT);
#endif

    if (r != OCI_SUCCESS && r != OCI_NEED_DATA) {
        return (ssize_t) -1;
    }

#if HAVE_OCILOBREAD2
    self->offset += self->csfrm == 0 ? byte_amt : char_amt;
#else
    self->offset += byte_amt;
#endif
    if (byte_amt < count) {
        stream->eof = 1;
    }
    return byte_amt;
}

static int oci_blob_close(php_stream *stream, int close_handle) {
    struct oci_lob_self *self = (struct oci_lob_self *) stream->abstract;
    pdo_stmt_t *stmt = self->stmt;

    if (close_handle) {
        zend_object *obj = &stmt->std;

        OCILobClose(self->E->svc, self->E->err, self->lob);
        zval_ptr_dtor(&self->dbh);
        GC_DELREF(obj);
        efree(self->E);
        efree(self);
    }

    /* php_pdo_free_statement(stmt); */
    return 0;
}

static int oci_blob_flush(php_stream *stream) {
    struct oci_lob_self *self = (struct oci_lob_self *) stream->abstract;
    OCILobFlushBuffer(self->E->svc, self->E->err, self->lob, 0);
    return 0;
}

static int oci_blob_seek(php_stream *stream, zend_off_t offset, int whence, zend_off_t *newoffset) {
    struct oci_lob_self *self = (struct oci_lob_self *) stream->abstract;

    if (offset >= PDO_OCI_LOBMAXSIZE) {
        return -1;
    } else {
        self->offset = (ub4) offset + 1; /* Oracle LOBS are 1-based, but PHP is 0-based */
        return 0;
    }
}

static const php_stream_ops oci_blob_stream_ops = {oci_blob_write,
                                                   oci_blob_read,
                                                   oci_blob_close,
                                                   oci_blob_flush,
                                                   "pdo_oci blob stream",
                                                   oci_blob_seek,
                                                   NULL,
                                                   NULL,
                                                   NULL};

static php_stream *oci_create_lob_stream(pdo_stmt_t *stmt, OCILobLocator *lob) {
    php_stream *stm;
    struct oci_lob_self *self = ecalloc(1, sizeof(*self));

#if PHP_API_VERSION < 20240925
    ZVAL_COPY_VALUE(&self->dbh, &stmt->database_object_handle);
#else
    ZVAL_OBJ(&self->dbh, stmt->database_object_handle);
#endif
    self->lob = lob;
    self->offset = 1; /* 1-based */
    self->stmt = stmt;
    self->S = (pdo_oci_stmt *) stmt->driver_data;
    self->E = ecalloc(1, sizeof(oci_lob_env));
    self->E->svc = swr_oci_svc(self->S->dbh);
    self->E->err = self->S->err;

    OCILobCharSetForm(swr_oci_env(self->S->dbh), self->S->err, self->lob, &self->csfrm);

    stm = php_stream_alloc(&oci_blob_stream_ops, self, 0, "r+b");

    if (stm) {
        zend_object *obj;
        obj = &stmt->std;
        Z_ADDREF(self->dbh);
        GC_ADDREF(obj);
        return stm;
    }

    efree(self);
    return NULL;
}

static int oci_stmt_get_col(pdo_stmt_t *stmt, int colno, zval *result, enum pdo_param_type *type) /* {{{ */
{
    pdo_oci_stmt *S = (pdo_oci_stmt *) stmt->driver_data;
    pdo_oci_column *C = &S->cols[colno];

    /* check the indicator to ensure that the data is intact */
    if (C->indicator == -1) {
        /* A NULL value */
        ZVAL_NULL(result);
        return 1;
    } else if (C->indicator == 0) {
        /* it was stored perfectly */

        if (C->dtype == SQLT_BLOB || C->dtype == SQLT_CLOB) {
            if (C->data) {
                php_stream *stream = oci_create_lob_stream(stmt, (OCILobLocator *) C->data);
                OCILobOpen(swr_oci_svc(S->dbh), S->err, (OCILobLocator *) C->data, OCI_LOB_READONLY);
                php_stream_to_zval(stream, result);
                return 1;
            }
            return 0;
        }

        ZVAL_STRINGL_FAST(result, C->data, C->fetched_len);
        return 1;
    } else {
        /* it was truncated */
        php_error_docref(NULL, E_WARNING, "Column %d data was too large for buffer and was truncated to fit it", colno);
        ZVAL_STRINGL(result, C->data, C->fetched_len);
        return 1;
    }
} /* }}} */

/* The statement's handles, which Rust reads (crates/oci/src/statement.rs). */
OCIStmt *swr_oci_stmt_handle(pdo_stmt_t *stmt) {
    return ((pdo_oci_stmt *) stmt->driver_data)->stmt;
}

OCIError *swr_oci_stmt_err(pdo_stmt_t *stmt) {
    return ((pdo_oci_stmt *) stmt->driver_data)->err;
}

extern void swr_oci_column_meta(pdo_stmt_t *stmt, pdo_dbh_t *dbh, uint32_t colno, zval *return_value);

/* PDO's method table takes a zend_result. */
static int oci_stmt_col_meta(pdo_stmt_t *stmt, zend_long colno, zval *return_value) /* {{{ */
{
    pdo_oci_stmt *S = (pdo_oci_stmt *) stmt->driver_data;
    if (!S->stmt) {
        return FAILURE;
    }
    if (colno >= stmt->column_count) {
        /* error invalid column */
        return FAILURE;
    }
    swr_oci_column_meta(stmt, stmt->dbh, (uint32_t) colno, return_value);
    return SUCCESS;
} /* }}} */

const struct pdo_stmt_methods swoole_oci_stmt_methods = {oci_stmt_dtor,
                                                         oci_stmt_execute,
                                                         oci_stmt_fetch,
                                                         oci_stmt_describe,
                                                         oci_stmt_get_col,
                                                         oci_stmt_param_hook,
                                                         NULL, /* set_attr */
                                                         NULL, /* get_attr */
                                                         oci_stmt_col_meta,
                                                         NULL,
                                                         NULL};

