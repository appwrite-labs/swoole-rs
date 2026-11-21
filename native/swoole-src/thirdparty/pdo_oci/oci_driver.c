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

struct swr_oci_error **swr_oci_stmt_einfo(pdo_stmt_t *stmt) {
    return &((pdo_oci_stmt *) stmt->driver_data)->einfo;
}

/* The connection is gone (crates/oci/src/handle.rs). */
extern void swr_oci_closed(pdo_dbh_t *dbh);

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

/* Rust closes the connection's handles and frees its record (crates/oci/src/driver.rs). */
extern void swr_oci_close(pdo_dbh_t *dbh);

void *swr_oci_take_handle(pdo_dbh_t *dbh) {
    void *handle = dbh->driver_data;
    dbh->driver_data = NULL;
    return handle;
}

static void oci_handle_closer(pdo_dbh_t *dbh) /* {{{ */
{
    swr_oci_close(dbh);
}
/* }}} */

static bool oci_handle_preparer(pdo_dbh_t *dbh, zend_string *sql, pdo_stmt_t *stmt, zval *driver_options) /* {{{ */
{
    pdo_oci_stmt *S = ecalloc(1, sizeof(*S));
    sword status;
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

    S->dbh = dbh;
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
    OCIHandleAlloc(swr_oci_env(dbh), (dvoid *) &S->stmt, OCI_HTYPE_STMT, 0, NULL);

    /* and our own private error handle */
    OCIHandleAlloc(swr_oci_env(dbh), (dvoid *) &S->err, OCI_HTYPE_ERROR, 0, NULL);

    if (ZSTR_LEN(sql) != 0) {
        status = OCIStmtPrepare(
            S->stmt, swr_oci_err(dbh), (text *) ZSTR_VAL(sql), (ub4) ZSTR_LEN(sql), OCI_NTV_SYNTAX, OCI_DEFAULT);
        if (nsql) {
            zend_string_release(nsql);
            nsql = NULL;
        }
        if (status) {
            _oci_error(swr_oci_err(dbh), dbh, NULL, "OCIStmtPrepare", status, FALSE, __FILE__, __LINE__);
            OCIHandleFree(S->stmt, OCI_HTYPE_STMT);
            OCIHandleFree(S->err, OCI_HTYPE_ERROR);
            efree(S);
            return false;
        }
    }

    prefetch = swr_oci_prefetch(dbh); /* Note 0 is allowed so in future REF CURSORs can be used & then passed with no row loss*/
    if (!OCIAttrSet(S->stmt, OCI_HTYPE_STMT, &prefetch, 0, OCI_ATTR_PREFETCH_ROWS, swr_oci_err(dbh))) {
        prefetch *= PDO_OCI_PREFETCH_ROWSIZE;
        OCIAttrSet(S->stmt, OCI_HTYPE_STMT, &prefetch, 0, OCI_ATTR_PREFETCH_MEMORY, swr_oci_err(dbh));
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

/* Rust opens the connection (crates/oci/src/handle.rs) from the data source PDO's
 * parser split, which C runs and frees. Stays C: private (the parser's records, whose
 * values PDO owns), allocator (the persistent user and password copies). */
extern bool swr_oci_connect(
    pdo_dbh_t *dbh, const char *charset, const char *dbname, const char *user, const char *password);

static int pdo_oci_handle_factory(pdo_dbh_t *dbh, zval *driver_options) /* {{{ */
{
    int i;
    bool connected;
    struct pdo_data_src_parser vars[] = {
        {"charset", NULL, 0}, {"dbname", "", 0}, {"user", NULL, 0}, {"password", NULL, 0}};

    php_pdo_parse_data_source(dbh->data_source, dbh->data_source_len, vars, 4);

    dbh->skip_param_evt = 1 << PDO_PARAM_EVT_FETCH_PRE | 1 << PDO_PARAM_EVT_FETCH_POST | 1 << PDO_PARAM_EVT_NORMALIZE;

    if (!dbh->username && vars[2].optval) {
        dbh->username = pestrdup(vars[2].optval, dbh->is_persistent);
    }
    if (!dbh->password && vars[3].optval) {
        dbh->password = pestrdup(vars[3].optval, dbh->is_persistent);
    }

    connected = swr_oci_connect(dbh, vars[0].optval, vars[1].optval, dbh->username, dbh->password);

    for (i = 0; i < sizeof(vars) / sizeof(vars[0]); i++) {
        if (vars[i].freeme) {
            efree(vars[i].optval);
        }
    }

    if (!connected) {
        oci_handle_closer(dbh);
        return 0;
    }

    dbh->methods = &oci_methods;
    dbh->alloc_own_columns = 1;
    dbh->native_case = PDO_CASE_UPPER;
    return 1;
}
/* }}} */

const pdo_driver_t swoole_pdo_oci_driver = {PDO_DRIVER_HEADER(oci), pdo_oci_handle_factory};


