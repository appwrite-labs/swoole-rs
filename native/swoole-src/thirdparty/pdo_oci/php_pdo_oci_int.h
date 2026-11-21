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

#ifndef PHP_PDO_OCI_INT_H
#define PHP_PDO_OCI_INT_H

#include "zend_portability.h"

#include <oci.h>

/* The last error of a handle or statement (crates/oci/src/error.rs), null until its first. */
struct swr_oci_error;

/* The connection's record is Rust's (crates/oci/src/handle.rs), read through these. */
OCISvcCtx *swr_oci_svc(pdo_dbh_t *dbh);
OCIError *swr_oci_err(pdo_dbh_t *dbh);
OCIEnv *swr_oci_env(pdo_dbh_t *dbh);
ub4 swr_oci_max_char_width(pdo_dbh_t *dbh);
ub4 swr_oci_prefetch(pdo_dbh_t *dbh);

typedef struct {
    OCIDefine *def;
    ub2 fetched_len;
    ub2 retcode;
    sb2 indicator;

    char *data;
    ub4 datalen;

    ub2 dtype;

} pdo_oci_column;

typedef struct {
    pdo_dbh_t *dbh;
    OCIStmt *stmt;
    OCIError *err;
    sword last_err;
    ub2 stmt_type;
    ub4 exec_type;
    pdo_oci_column *cols;
    struct swr_oci_error *einfo;
    unsigned int have_blobs : 1;
} pdo_oci_stmt;

typedef struct {
    OCIBind *bind; /* allocated by OCI */
    sb2 oci_type;
    sb2 indicator;
    ub2 retcode;

    ub4 actual_len;

    dvoid *thing; /* for LOBS, REFCURSORS etc. */

    unsigned used_for_output;

    /* --- NEW: preconverted buffer for async-safe binds --- */
     char *preconv_buf;      /* pointer to a NUL-terminated C buffer (owned by param) */
     ub4   preconv_len;      /* length of the buffer in bytes */
} pdo_oci_bound_param;

extern const ub4 SWOOLE_PDO_OCI_INIT_MODE;
extern const pdo_driver_t swoole_pdo_oci_driver;
extern OCIEnv *swoole_pdo_oci_Env;

ub4 _oci_error(
    OCIError *err, pdo_dbh_t *dbh, pdo_stmt_t *stmt, char *what, sword status, int isinit, const char *file, int line);
/* Free a statement's last error. */
void swr_oci_einfo_free(pdo_dbh_t *dbh, pdo_stmt_t *stmt);
#define oci_stmt_error(w) _oci_error(S->err, stmt->dbh, stmt, w, S->last_err, FALSE, __FILE__, __LINE__)

extern const struct pdo_stmt_methods swoole_oci_stmt_methods;
extern const ub4 SWOOLE_PDO_OCI_INIT_MODE;
extern OCIEnv *swoole_pdo_oci_Env;

/* Default prefetch size in number of rows */
#define PDO_OCI_PREFETCH_DEFAULT 100

/* Arbitrary assumed row length for prefetch memory limit calcuation */
#define PDO_OCI_PREFETCH_ROWSIZE 1024

enum {
    PDO_OCI_ATTR_ACTION = PDO_ATTR_DRIVER_SPECIFIC,
    PDO_OCI_ATTR_CLIENT_INFO,
    PDO_OCI_ATTR_CLIENT_IDENTIFIER,
    PDO_OCI_ATTR_MODULE,
    PDO_OCI_ATTR_CALL_TIMEOUT
};

#endif /* PHP_PDO_OCI_INT_H */
