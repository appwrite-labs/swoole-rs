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

/* pdo_oci's statement side. crates/oci/src/statement.rs, columns.rs, bind.rs and lob.rs
 * have the operations.
 *
 * Stays C (docs/zend.md, What stays C): allocator (the statement record and the LOB
 * stream's state are Zend's to allocate and free), bailout (reading a bound stream into
 * a LOB and closing a LOB stream can run PHP code), globals (the destructor checks the
 * connection object in EG(objects_store)) and budget (pdo_oci_stmt's fields, read
 * through the accessors here). The method table and the LOB stream's operations are
 * C because PDO and the streams layer declare them over pdo_stmt_t and php_stream
 * pointers, which the C compiler checks; OCI's dynamic-bind callbacks return its sb4. */
#include "php_swoole_oracle.h"

#ifdef SW_USE_ORACLE
#include "php.h"
#include "php_ini.h"
#include "ext/standard/info.h"
#include "pdo/php_pdo.h"
#include "Zend/zend_extensions.h"

#define PDO_OCI_LOBMAXSIZE (4294967295UL) /* OCI_LOBMAXSIZE */

static php_stream *oci_create_lob_stream(pdo_stmt_t *stmt, OCILobLocator *lob);

/* The columns are Rust's (crates/oci/src/columns.rs). */
extern void swr_oci_columns_free(pdo_stmt_t *stmt, pdo_dbh_t *dbh, bool usable);
extern bool swr_oci_describe(pdo_stmt_t *stmt, pdo_dbh_t *dbh, uint32_t colno, struct pdo_column_data *desc);
extern bool swr_oci_column_value(pdo_stmt_t *stmt, uint32_t colno, zval *result);
extern bool swr_oci_execute(pdo_stmt_t *stmt, pdo_dbh_t *dbh);
extern void swr_oci_stmt_close(pdo_stmt_t *stmt);

/* What Rust reads of the statement record (crates/oci/src/statement.rs). */
ub2 swr_oci_stmt_type(pdo_stmt_t *stmt) {
    return ((pdo_oci_stmt *) stmt->driver_data)->stmt_type;
}

void swr_oci_set_stmt_type(pdo_stmt_t *stmt, ub2 kind) {
    ((pdo_oci_stmt *) stmt->driver_data)->stmt_type = kind;
}

bool swr_oci_stmt_scrollable(pdo_stmt_t *stmt) {
    return ((pdo_oci_stmt *) stmt->driver_data)->exec_type == OCI_STMT_SCROLLABLE_READONLY;
}

bool swr_oci_stmt_have_blobs(pdo_stmt_t *stmt) {
    return ((pdo_oci_stmt *) stmt->driver_data)->have_blobs;
}

struct swr_oci_columns **swr_oci_stmt_columns(pdo_stmt_t *stmt) {
    return &((pdo_oci_stmt *) stmt->driver_data)->cols;
}

/* A column's name, read where OCI returns it as a pointer and length. Stays C:
 * budget (reading those bytes from Rust is an unsafe block). */
zend_string *swr_oci_param_name(OCIParam *param, OCIError *err, sword *status) {
    text *name = NULL;
    ub4 length = 0;
    *status = OCIAttrGet(param, OCI_DTYPE_PARAM, &name, &length, OCI_ATTR_NAME, err);
    return *status == OCI_SUCCESS && name ? zend_string_init((char *) name, length, 0) : NULL;
}

static int oci_stmt_dtor(pdo_stmt_t *stmt) /* {{{ */
{
    pdo_oci_stmt *S = (pdo_oci_stmt *) stmt->driver_data;
    HashTable *BC = stmt->bound_columns;
    HashTable *BP = stmt->bound_params;

    swr_oci_stmt_close(stmt);

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

    swr_oci_columns_free(stmt, stmt->dbh, server_obj_usable);
    efree(S);

    stmt->driver_data = NULL;

    return 1;
} /* }}} */

/* PDO's method table takes an int; Rust runs the statement. */
static int oci_stmt_execute(pdo_stmt_t *stmt) /* {{{ */
{
    return swr_oci_execute(stmt, stmt->dbh) ? 1 : 0;
} /* }}} */

/* The bound parameters are Rust (crates/oci/src/bind.rs), each record boxed in
 * param->driver_data. OCI calls these during OCIStmtExecute, on the thread pool
 * under the coroutine hook; they hand Rust only that record, and return the sb4
 * OCI takes. */
extern void swr_oci_bind_input(void *bound, dvoid **bufpp, ub4 *alenp, ub1 *piecep, dvoid **indpp);
extern void swr_oci_bind_output(void *bound, dvoid **bufpp, ub4 **alenpp, ub1 *piecep, dvoid **indpp, ub2 **rcodepp);

sb4 swr_oci_bind_input_cb(
    dvoid *ctx, OCIBind *bindp, ub4 iter, ub4 index, dvoid **bufpp, ub4 *alenp, ub1 *piecep, dvoid **indpp) {
    swr_oci_bind_input(ctx, bufpp, alenp, piecep, indpp);
    return OCI_CONTINUE;
}

sb4 swr_oci_bind_output_cb(dvoid *ctx,
                           OCIBind *bindp,
                           ub4 iter,
                           ub4 index,
                           dvoid **bufpp,
                           ub4 **alenpp,
                           ub1 *piecep,
                           dvoid **indpp,
                           ub2 **rcodepp) {
    swr_oci_bind_output(ctx, bufpp, alenpp, piecep, indpp, rcodepp);
    return OCI_CONTINUE;
}

extern bool swr_oci_param_event(pdo_stmt_t *stmt,
                                pdo_dbh_t *dbh,
                                enum pdo_param_event event,
                                enum pdo_param_type kind,
                                zend_long max_value_len,
                                zend_string *name,
                                zend_long paramno,
                                void **data,
                                zval *value);

/* PDO's method table takes an int; Rust handles the event. */
static int oci_stmt_param_hook(pdo_stmt_t *stmt,
                               struct pdo_bound_param_data *param,
                               enum pdo_param_event event_type) /* {{{ */
{
    zval *value = &param->parameter;
    if (!param->is_param) {
        return 1;
    }
    ZVAL_DEREF(value);
    return swr_oci_param_event(stmt,
                               stmt->dbh,
                               event_type,
                               PDO_PARAM_TYPE(param->param_type),
                               param->max_value_len,
                               param->name,
                               param->paramno,
                               &param->driver_data,
                               value)
               ? 1
               : 0;
} /* }}} */

void swr_oci_set_have_blobs(pdo_stmt_t *stmt) {
    ((pdo_oci_stmt *) stmt->driver_data)->have_blobs = 1;
}

/* A NULL output: the parameter released and left undefined, as C left it. */
void swr_oci_param_undef(zval *value) {
    zval_ptr_dtor(value);
    ZVAL_UNDEF(value);
}

/* A LOB locator bound to a NULL: the query's LOB as a read-write stream. */
void swr_oci_lob_stream(pdo_stmt_t *stmt, OCILobLocator *lob, zval *value) {
    pdo_oci_stmt *S = (pdo_oci_stmt *) stmt->driver_data;
    php_stream *stm = oci_create_lob_stream(stmt, lob);
    if (stm) {
        OCILobOpen(swr_oci_svc(S->dbh), S->err, lob, OCI_LOB_READWRITE);
        php_stream_to_zval(stm, value);
    }
}

/* A LOB being inserted: the stream's data, or the string, written to it. Stays C:
 * php_stream_from_zval_no_verify's resource fetch, and reading the stream. */
void swr_oci_lob_fill(pdo_stmt_t *stmt, OCILobLocator *lob, zval *parameter) {
    pdo_oci_stmt *S = (pdo_oci_stmt *) stmt->driver_data;
    php_stream *stm;
    size_t n;
    ub4 amt, offset = 1;
    char *consume;

    php_stream_from_zval_no_verify(stm, parameter);
    if (stm) {
        OCILobOpen(swr_oci_svc(S->dbh), S->err, lob, OCI_LOB_READWRITE);
        do {
            char buf[8192];
            n = php_stream_read(stm, buf, sizeof(buf));
            if ((int) n <= 0) {
                break;
            }
            consume = buf;
            do {
                amt = (ub4) n;
                OCILobWrite(swr_oci_svc(S->dbh), S->err, lob, &amt, offset, consume, (ub4) n, OCI_ONE_PIECE, NULL,
                            NULL, 0, SQLCS_IMPLICIT);
                offset += amt;
                n -= amt;
                consume += amt;
            } while (n);
        } while (1);
        OCILobClose(swr_oci_svc(S->dbh), S->err, lob);
        OCILobFlushBuffer(swr_oci_svc(S->dbh), S->err, lob, 0);
    } else if (Z_TYPE_P(parameter) == IS_STRING) {
        /* stick the string into the LOB */
        consume = Z_STRVAL_P(parameter);
        n = Z_STRLEN_P(parameter);
        if (n) {
            OCILobOpen(swr_oci_svc(S->dbh), S->err, lob, OCI_LOB_READWRITE);
            while (n) {
                amt = (ub4) n;
                OCILobWrite(swr_oci_svc(S->dbh), S->err, lob, &amt, offset, consume, (ub4) n, OCI_ONE_PIECE, NULL,
                            NULL, 0, SQLCS_IMPLICIT);
                consume += amt;
                n -= amt;
            }
            OCILobClose(swr_oci_svc(S->dbh), S->err, lob);
        }
    }
}

/* Rust fetches the row (crates/oci/src/statement.rs). PDO's method table takes an int. */
extern bool swr_oci_fetch(pdo_stmt_t *stmt, pdo_dbh_t *dbh, uint32_t orientation, zend_long offset);

static int oci_stmt_fetch(pdo_stmt_t *stmt, enum pdo_fetch_orientation ori, zend_long offset) /* {{{ */
{
    return swr_oci_fetch(stmt, stmt->dbh, (uint32_t) ori, offset) ? 1 : 0;
} /* }}} */

/**
 * This function is thread-safe as it does not call any ZendAPI.
 */

/* PDO's method table takes an int; Rust describes and defines the column. */
static int oci_stmt_describe(pdo_stmt_t *stmt, int colno) /* {{{ */
{
    return swr_oci_describe(stmt, stmt->dbh, (uint32_t) colno, &stmt->columns[colno]) ? 1 : 0;
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

/* The stream's operations are Rust's (crates/oci/src/lob.rs); these hand them the
 * stream's state. The stream, its state and its references stay C. */
extern size_t swr_oci_lob_write(OCISvcCtx *svc, OCIError *err, OCILobLocator *lob, ub4 *offset, const char *buf, ub4 count);
extern size_t swr_oci_lob_read(OCISvcCtx *svc, OCIError *err, OCILobLocator *lob, ub4 *offset, ub1 form, char *buf,
                               oraub8 count, bool *eof);
extern void swr_oci_lob_flush(OCISvcCtx *svc, OCIError *err, OCILobLocator *lob);
extern void swr_oci_lob_close(OCISvcCtx *svc, OCIError *err, OCILobLocator *lob);
extern bool swr_oci_lob_seek(int64_t offset, ub4 *position);
extern ub1 swr_oci_lob_form(OCIEnv *env, OCIError *err, OCILobLocator *lob);

static ssize_t oci_blob_write(php_stream *stream, const char *buf, size_t count) {
    struct oci_lob_self *self = (struct oci_lob_self *) stream->abstract;
    size_t wrote = swr_oci_lob_write(self->E->svc, self->E->err, self->lob, &self->offset, buf, (ub4) count);
    return wrote == SIZE_MAX ? (ssize_t) -1 : (ssize_t) wrote;
}

static ssize_t oci_blob_read(php_stream *stream, char *buf, size_t count) {
    struct oci_lob_self *self = (struct oci_lob_self *) stream->abstract;
    bool eof = false;
    size_t read = swr_oci_lob_read(self->E->svc, self->E->err, self->lob, &self->offset, self->csfrm, buf, (oraub8) count, &eof);
    if (read == SIZE_MAX) {
        return (ssize_t) -1;
    }
    if (eof) {
        stream->eof = 1;
    }
    return (ssize_t) read;
}

static int oci_blob_close(php_stream *stream, int close_handle) {
    struct oci_lob_self *self = (struct oci_lob_self *) stream->abstract;
    pdo_stmt_t *stmt = self->stmt;

    if (close_handle) {
        zend_object *obj = &stmt->std;

        swr_oci_lob_close(self->E->svc, self->E->err, self->lob);
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
    swr_oci_lob_flush(self->E->svc, self->E->err, self->lob);
    return 0;
}

static int oci_blob_seek(php_stream *stream, zend_off_t offset, int whence, zend_off_t *newoffset) {
    struct oci_lob_self *self = (struct oci_lob_self *) stream->abstract;
    return swr_oci_lob_seek(offset, &self->offset) ? 0 : -1;
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

    self->csfrm = swr_oci_lob_form(swr_oci_env(self->S->dbh), self->S->err, self->lob);

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

/* A LOB column's value (crates/oci/src/columns.rs): its stream, opened read-only. */
void swr_oci_lob_value(pdo_stmt_t *stmt, OCILobLocator *lob, zval *result) {
    pdo_oci_stmt *S = (pdo_oci_stmt *) stmt->driver_data;
    php_stream *stream = oci_create_lob_stream(stmt, lob);
    OCILobOpen(swr_oci_svc(S->dbh), S->err, lob, OCI_LOB_READONLY);
    php_stream_to_zval(stream, result);
}

/* PDO's method table takes an int; Rust reads the fetched value. */
static int oci_stmt_get_col(pdo_stmt_t *stmt, int colno, zval *result, enum pdo_param_type *type) /* {{{ */
{
    return swr_oci_column_value(stmt, (uint32_t) colno, result) ? 1 : 0;
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

#endif /* SW_USE_ORACLE */
