/* The coroutine OCI calls crates/oci exports (--with-swoole-oracle), and the
 * macros that point the bundled pdo_oci driver at them. The driver includes
 * this first and gets its php_pdo_oci_int.h (oci.h and the driver's types) from
 * here. */
#ifndef PHP_SWOOLE_ORACLE_H
#define PHP_SWOOLE_ORACLE_H
#include "php_swoole.h"
#ifdef SW_USE_ORACLE
BEGIN_EXTERN_C()
#include "ext/pdo/php_pdo_driver.h"
#include "thirdparty/pdo_oci/php_pdo_oci_int.h"
sword swoole_oci_session_begin(OCISvcCtx *svchp, OCIError *errhp, OCISession *usrhp, ub4 credt, ub4 mode);
sword swoole_oci_server_detach(OCIServer *srvhp, OCIError *errhp, ub4 mode);
sword swoole_oci_stmt_prepare(OCIStmt *stmtp, OCIError *errhp, const OraText *stmt, ub4 stmt_len, ub4 language,
                              ub4 mode);
sword swoole_oci_stmt_execute(OCISvcCtx *svchp, OCIStmt *stmtp, OCIError *errhp, ub4 iters, ub4 rowoff,
                              const OCISnapshot *snap_in, OCISnapshot *snap_out, ub4 mode);
sword swoole_oci_stmt_fetch(OCIStmt *stmtp, OCIError *errhp, ub4 nrows, ub2 orientation, ub4 mode);
sword swoole_oci_stmt_fetch2(OCIStmt *stmtp, OCIError *errhp, ub4 nrows, ub2 orientation, sb4 scrollOffset, ub4 mode);
sword swoole_oci_trans_commit(OCISvcCtx *svchp, OCIError *errhp, ub4 flags);
sword swoole_oci_trans_rollback(OCISvcCtx *svchp, OCIError *errhp, ub4 flags);
sword swoole_oci_ping(OCISvcCtx *svchp, OCIError *errhp, ub4 mode);
END_EXTERN_C()
#define OCISessionBegin swoole_oci_session_begin
#define OCIServerDetach swoole_oci_server_detach
#define OCIStmtPrepare swoole_oci_stmt_prepare
#define OCIStmtExecute swoole_oci_stmt_execute
#define OCIStmtFetch swoole_oci_stmt_fetch
#define OCIStmtFetch2 swoole_oci_stmt_fetch2
#define OCITransCommit swoole_oci_trans_commit
#define OCITransRollback swoole_oci_trans_rollback
#define OCIPing swoole_oci_ping
#endif
#endif
