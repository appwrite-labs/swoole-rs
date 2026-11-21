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
#include "Zend/zend_exceptions.h"


/* Rust records the error (crates/oci/src/error.rs). Stays C: bailout (ORA-01013,
 * the user cancelled the call, ends the request) and the handle's attached bit. */
ub4 swr_oci_error(OCIError *err, pdo_dbh_t *dbh, pdo_stmt_t *stmt, char *what, sword status, bool isinit,
                  const char *file, int line, bool *closed, bool *cancelled);
void pdo_oci_fetch_error_func(pdo_dbh_t *dbh, pdo_stmt_t *stmt, zval *info);

struct swr_oci_error **swr_oci_einfo(pdo_dbh_t *dbh, pdo_stmt_t *stmt) {
    if (stmt) {
        return &((pdo_oci_stmt *) stmt->driver_data)->einfo;
    }
    return &((pdo_oci_db_handle *) dbh->driver_data)->einfo;
}

/* The connection is gone. */
void swr_oci_closed(pdo_dbh_t *dbh) {
    dbh->is_closed = 1;
    ((pdo_oci_db_handle *) dbh->driver_data)->attached = 0;
}

ub4 _oci_error(OCIError *err,
               pdo_dbh_t *dbh,
               pdo_stmt_t *stmt,
               char *what,
               sword status,
               int isinit,
               const char *file,
               int line) /* {{{ */
{
    bool closed = false, cancelled = false;
    ub4 code = swr_oci_error(err, dbh, stmt, what, status, isinit, file, line, &closed, &cancelled);
    if (cancelled) {
        zend_bailout();
    }
    if (closed) {
        swr_oci_closed(dbh);
    }
    return code;
}
/* }}} */

static void oci_handle_closer(pdo_dbh_t *dbh) /* {{{ */
{
    pdo_oci_db_handle *H = (pdo_oci_db_handle *) dbh->driver_data;

    if (H->svc) {
        /* rollback any outstanding work */
        OCITransRollback(H->svc, H->err, 0);
    }

    if (H->session) {
        OCIHandleFree(H->session, OCI_HTYPE_SESSION);
        H->session = NULL;
    }

    if (H->svc) {
        OCIHandleFree(H->svc, OCI_HTYPE_SVCCTX);
        H->svc = NULL;
    }

    if (H->server && H->attached) {
        H->last_err = OCIServerDetach(H->server, H->err, OCI_DEFAULT);
        if (H->last_err) {
            oci_drv_error("OCIServerDetach");
        }
        H->attached = 0;
    }

    if (H->server) {
        OCIHandleFree(H->server, OCI_HTYPE_SERVER);
        H->server = NULL;
    }

    if (H->err) {
        OCIHandleFree(H->err, OCI_HTYPE_ERROR);
        H->err = NULL;
    }

    if (H->charset && H->env) {
        OCIHandleFree(H->env, OCI_HTYPE_ENV);
        H->env = NULL;
    }

    swr_oci_einfo_free(dbh, NULL);

    pefree(H, dbh->is_persistent);
}
/* }}} */

static bool oci_handle_preparer(pdo_dbh_t *dbh, zend_string *sql, pdo_stmt_t *stmt, zval *driver_options) /* {{{ */
{
    pdo_oci_db_handle *H = (pdo_oci_db_handle *) dbh->driver_data;
    pdo_oci_stmt *S = ecalloc(1, sizeof(*S));
    ub4 prefetch;
    zend_string *nsql = NULL;
    int ret;

#ifdef HAVE_OCISTMTFETCH2
    S->exec_type = pdo_attr_lval(driver_options, PDO_ATTR_CURSOR, PDO_CURSOR_FWDONLY) == PDO_CURSOR_SCROLL
                       ? OCI_STMT_SCROLLABLE_READONLY
                       : OCI_DEFAULT;
#else
    S->exec_type = OCI_DEFAULT;
#endif

    S->H = H;
    stmt->supports_placeholders = PDO_PLACEHOLDER_NAMED;
    ret = pdo_parse_params(stmt, sql, &nsql);

    if (ret == 1) {
        /* query was re-written */
        sql = nsql;
    } else if (ret == -1) {
        /* couldn't grok it */
        strcpy(dbh->error_code, stmt->error_code);
        efree(S);
        return false;
    }

    /* create an OCI statement handle */
    OCIHandleAlloc(H->env, (dvoid *) &S->stmt, OCI_HTYPE_STMT, 0, NULL);

    /* and our own private error handle */
    OCIHandleAlloc(H->env, (dvoid *) &S->err, OCI_HTYPE_ERROR, 0, NULL);

    if (ZSTR_LEN(sql) != 0) {
        H->last_err =
            OCIStmtPrepare(S->stmt, H->err, (text *) ZSTR_VAL(sql), (ub4) ZSTR_LEN(sql), OCI_NTV_SYNTAX, OCI_DEFAULT);
        if (nsql) {
            zend_string_release(nsql);
            nsql = NULL;
        }
        if (H->last_err) {
            H->last_err = oci_drv_error("OCIStmtPrepare");
            OCIHandleFree(S->stmt, OCI_HTYPE_STMT);
            OCIHandleFree(S->err, OCI_HTYPE_ERROR);
            efree(S);
            return false;
        }
    }

    prefetch = H->prefetch; /* Note 0 is allowed so in future REF CURSORs can be used & then passed with no row loss*/
    H->last_err = OCIAttrSet(S->stmt, OCI_HTYPE_STMT, &prefetch, 0, OCI_ATTR_PREFETCH_ROWS, H->err);
    if (!H->last_err) {
        prefetch *= PDO_OCI_PREFETCH_ROWSIZE;
        H->last_err = OCIAttrSet(S->stmt, OCI_HTYPE_STMT, &prefetch, 0, OCI_ATTR_PREFETCH_MEMORY, H->err);
    }

    stmt->driver_data = S;
    stmt->methods = &swoole_oci_stmt_methods;
    if (nsql) {
        zend_string_release(nsql);
        nsql = NULL;
    }

    return true;
}
/* }}} */

/* The connection's methods in Rust (crates/oci/src/driver.rs), and what they read of the handle. */
extern bool swr_oci_begin(pdo_dbh_t *dbh);
extern bool swr_oci_commit(pdo_dbh_t *dbh);
extern bool swr_oci_rollback(pdo_dbh_t *dbh);
extern bool swr_oci_alive(pdo_dbh_t *dbh);
extern int swr_oci_get_attribute(pdo_dbh_t *dbh, zend_long attr, zval *return_value);
extern bool swr_oci_set_attribute(pdo_dbh_t *dbh, zend_long attr, zval *val);
extern zend_long swr_oci_exec(pdo_dbh_t *dbh, const zend_string *sql);
extern zend_string *swr_oci_quote(pdo_dbh_t *dbh, const zend_string *unquoted, enum pdo_param_type paramtype);

OCISvcCtx *swr_oci_svc(pdo_dbh_t *dbh) {
    return ((pdo_oci_db_handle *) dbh->driver_data)->svc;
}

OCIError *swr_oci_err(pdo_dbh_t *dbh) {
    return ((pdo_oci_db_handle *) dbh->driver_data)->err;
}

bool swr_oci_attached(pdo_dbh_t *dbh) {
    return ((pdo_oci_db_handle *) dbh->driver_data)->attached;
}

ub4 swr_oci_prefetch(pdo_dbh_t *dbh) {
    return ((pdo_oci_db_handle *) dbh->driver_data)->prefetch;
}

void swr_oci_set_prefetch(pdo_dbh_t *dbh, ub4 rows) {
    ((pdo_oci_db_handle *) dbh->driver_data)->prefetch = rows;
}

OCIEnv *swr_oci_env(pdo_dbh_t *dbh) {
    return ((pdo_oci_db_handle *) dbh->driver_data)->env;
}

OCISession *swr_oci_session(pdo_dbh_t *dbh) {
    return ((pdo_oci_db_handle *) dbh->driver_data)->session;
}

/* PDO's method table takes a zend_result. */
static zend_result pdo_oci_check_liveness(pdo_dbh_t *dbh) {
    return swr_oci_alive(dbh) ? SUCCESS : FAILURE;
}

static const struct pdo_dbh_methods oci_methods = {
    oci_handle_closer,
    oci_handle_preparer,
    swr_oci_exec,
    swr_oci_quote,
    swr_oci_begin,
    swr_oci_commit,
    swr_oci_rollback,
    swr_oci_set_attribute,
    NULL, /* last_id not supported */
    pdo_oci_fetch_error_func,
    swr_oci_get_attribute,
    pdo_oci_check_liveness, /* check_liveness */
    NULL,                   /* get_driver_methods */
    NULL,                   /* request_shutdown */
    NULL,                   /* in transaction, use PDO's internal tracking mechanism */
    NULL                    /* get_gc */
};

static int pdo_oci_handle_factory(pdo_dbh_t *dbh, zval *driver_options) /* {{{ */
{
    pdo_oci_db_handle *H;
    int i, ret = 0;
    struct pdo_data_src_parser vars[] = {
        {"charset", NULL, 0}, {"dbname", "", 0}, {"user", NULL, 0}, {"password", NULL, 0}};

    php_pdo_parse_data_source(dbh->data_source, dbh->data_source_len, vars, 4);

    H = pecalloc(1, sizeof(*H), dbh->is_persistent);
    dbh->driver_data = H;

    dbh->skip_param_evt = 1 << PDO_PARAM_EVT_FETCH_PRE | 1 << PDO_PARAM_EVT_FETCH_POST | 1 << PDO_PARAM_EVT_NORMALIZE;

    H->prefetch = PDO_OCI_PREFETCH_DEFAULT;

    /* allocate an environment */
#ifdef HAVE_OCIENVNLSCREATE
    if (vars[0].optval) {
        H->charset = OCINlsCharSetNameToId(swoole_pdo_oci_Env, (const oratext *) vars[0].optval);
        if (!H->charset) {
            oci_init_error("OCINlsCharSetNameToId: unknown character set name");
            goto cleanup;
        } else {
            if (OCIEnvNlsCreate(&H->env, SWOOLE_PDO_OCI_INIT_MODE, 0, NULL, NULL, NULL, 0, NULL, H->charset, H->charset) !=
                OCI_SUCCESS) {
                oci_init_error("OCIEnvNlsCreate: Check the character set is valid and that PHP has access to Oracle "
                               "libraries and NLS data");
                goto cleanup;
            }
        }
    }
#endif
    if (H->env == NULL) {
        /* use the global environment */
        H->env = swoole_pdo_oci_Env;
    }

    /* something to hold errors */
    OCIHandleAlloc(H->env, (dvoid **) &H->err, OCI_HTYPE_ERROR, 0, NULL);

    /* handle for the server */
    OCIHandleAlloc(H->env, (dvoid **) &H->server, OCI_HTYPE_SERVER, 0, NULL);

    H->last_err =
        OCIServerAttach(H->server, H->err, (text *) vars[1].optval, (sb4) strlen(vars[1].optval), OCI_DEFAULT);

    if (H->last_err) {
        oci_drv_error("pdo_oci_handle_factory");
        goto cleanup;
    }

    H->attached = 1;

    /* create a service context */
    H->last_err = OCIHandleAlloc(H->env, (dvoid **) &H->svc, OCI_HTYPE_SVCCTX, 0, NULL);
    if (H->last_err) {
        oci_drv_error("OCIHandleAlloc: OCI_HTYPE_SVCCTX");
        goto cleanup;
    }

    H->last_err = OCIHandleAlloc(H->env, (dvoid **) &H->session, OCI_HTYPE_SESSION, 0, NULL);
    if (H->last_err) {
        oci_drv_error("OCIHandleAlloc: OCI_HTYPE_SESSION");
        goto cleanup;
    }

    /* set server handle into service handle */
    H->last_err = OCIAttrSet(H->svc, OCI_HTYPE_SVCCTX, H->server, 0, OCI_ATTR_SERVER, H->err);
    if (H->last_err) {
        oci_drv_error("OCIAttrSet: OCI_ATTR_SERVER");
        goto cleanup;
    }

    /* username */
    if (!dbh->username && vars[2].optval) {
        dbh->username = pestrdup(vars[2].optval, dbh->is_persistent);
    }

    if (dbh->username) {
        H->last_err = OCIAttrSet(
            H->session, OCI_HTYPE_SESSION, dbh->username, (ub4) strlen(dbh->username), OCI_ATTR_USERNAME, H->err);
        if (H->last_err) {
            oci_drv_error("OCIAttrSet: OCI_ATTR_USERNAME");
            goto cleanup;
        }
    }

    /* password */
    if (!dbh->password && vars[3].optval) {
        dbh->password = pestrdup(vars[3].optval, dbh->is_persistent);
    }

    if (dbh->password) {
        H->last_err = OCIAttrSet(
            H->session, OCI_HTYPE_SESSION, dbh->password, (ub4) strlen(dbh->password), OCI_ATTR_PASSWORD, H->err);
        if (H->last_err) {
            oci_drv_error("OCIAttrSet: OCI_ATTR_PASSWORD");
            goto cleanup;
        }
    }

    /* Now fire up the session */
    H->last_err = OCISessionBegin(H->svc, H->err, H->session, OCI_CRED_RDBMS, OCI_DEFAULT);
    if (H->last_err) {
        oci_drv_error("OCISessionBegin");
        /* OCISessionBegin returns OCI_SUCCESS_WITH_INFO when
         * user's password has expired, but is still usable.
         */
        if (H->last_err != OCI_SUCCESS_WITH_INFO) {
            goto cleanup;
        }
    }

    /* set the server handle into service handle */
    H->last_err = OCIAttrSet(H->svc, OCI_HTYPE_SVCCTX, H->session, 0, OCI_ATTR_SESSION, H->err);
    if (H->last_err) {
        oci_drv_error("OCIAttrSet: OCI_ATTR_SESSION");
        goto cleanup;
    }

    /* Get max character width */
    H->last_err = OCINlsNumericInfoGet(H->env, H->err, &H->max_char_width, OCI_NLS_CHARSET_MAXBYTESZ);
    if (H->last_err) {
        oci_drv_error("OCINlsNumericInfoGet: OCI_NLS_CHARSET_MAXBYTESZ");
        goto cleanup;
    }

    dbh->methods = &oci_methods;
    dbh->alloc_own_columns = 1;
    dbh->native_case = PDO_CASE_UPPER;

    ret = 1;

cleanup:
    for (i = 0; i < sizeof(vars) / sizeof(vars[0]); i++) {
        if (vars[i].freeme) {
            efree(vars[i].optval);
        }
    }

    if (!ret) {
        oci_handle_closer(dbh);
    }

    return ret;
}
/* }}} */

const pdo_driver_t swoole_pdo_oci_driver = {PDO_DRIVER_HEADER(oci), pdo_oci_handle_factory};


