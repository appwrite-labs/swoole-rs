/* The coroutine ODBC calls crates/odbc exports (--with-swoole-odbc), and the
 * macros that point the bundled pdo_odbc driver at them. The driver includes
 * this first, after defining SW_USE_ODBC_HOOK, and gets its php_pdo_odbc_int.h
 * (the ODBC flavour's headers) from here. */
#ifndef PHP_SWOOLE_ODBC_H
#define PHP_SWOOLE_ODBC_H
#include "php_swoole.h"
#ifdef SW_USE_ODBC
BEGIN_EXTERN_C()
#include "ext/pdo/php_pdo_driver.h"
#if PHP_VERSION_ID < 80300
#include "thirdparty/php82/pdo_odbc/php_pdo_odbc_int.h"
#elif PHP_VERSION_ID < 80400
#include "thirdparty/php83/pdo_odbc/php_pdo_odbc_int.h"
#elif PHP_VERSION_ID < 80500
#include "thirdparty/php84/pdo_odbc/php_pdo_odbc_int.h"
#else
#include "thirdparty/php85/pdo_odbc/php_pdo_odbc_int.h"
#endif
RETCODE swoole_odbc_SQLConnect(SQLHDBC dbc, SQLCHAR *server, SQLSMALLINT server_len, SQLCHAR *user, SQLSMALLINT user_len,
                               SQLCHAR *password, SQLSMALLINT password_len);
SQLRETURN SQL_API swoole_odbc_SQLDriverConnect(SQLHDBC dbc, SQLHWND hwnd, SQLCHAR *in, SQLSMALLINT in_len, SQLCHAR *out,
                                               SQLSMALLINT out_max, SQLSMALLINT *out_len, SQLUSMALLINT completion);
SQLRETURN SQL_API swoole_odbc_SQLExecDirect(SQLHSTMT stmt, SQLCHAR *text, SQLINTEGER text_len);
SQLRETURN SQL_API swoole_odbc_SQLGetInfo(SQLHDBC dbc, SQLUSMALLINT type, SQLPOINTER value, SQLSMALLINT buffer_len,
                                         SQLSMALLINT *len);
SQLRETURN SQL_API swoole_odbc_SQLGetDiagRec(SQLSMALLINT handle_type, SQLHANDLE handle, SQLSMALLINT record,
                                            SQLCHAR *sqlstate, SQLINTEGER *native_error, SQLCHAR *message,
                                            SQLSMALLINT buffer_len, SQLSMALLINT *len);
SQLRETURN SQL_API swoole_odbc_SQLPrepare(SQLHSTMT stmt, SQLCHAR *text, SQLINTEGER text_len);
SQLRETURN SQL_API swoole_odbc_SQLExecute(SQLHSTMT stmt);
SQLRETURN SQL_API swoole_odbc_SQLCloseCursor(SQLHSTMT stmt);
SQLRETURN SQL_API swoole_odbc_SQLPutData(SQLHSTMT stmt, SQLPOINTER data, SQLLEN len_or_ind);
SQLRETURN SQL_API swoole_odbc_SQLGetData(SQLHSTMT stmt, SQLUSMALLINT column, SQLSMALLINT target_type, SQLPOINTER target,
                                         SQLLEN buffer_len, SQLLEN *len_or_ind);
SQLRETURN SQL_API swoole_odbc_SQLRowCount(SQLHSTMT stmt, SQLLEN *rows);
SQLRETURN SQL_API swoole_odbc_SQLDescribeCol(SQLHSTMT stmt, SQLUSMALLINT column, SQLCHAR *name, SQLSMALLINT buffer_len,
                                             SQLSMALLINT *name_len, SQLSMALLINT *data_type, SQLULEN *size,
                                             SQLSMALLINT *decimal_digits, SQLSMALLINT *nullable);
SQLRETURN SQL_API swoole_odbc_SQLMoreResults(SQLHSTMT stmt);
SQLRETURN SQL_API swoole_odbc_SQLEndTran(SQLSMALLINT handle_type, SQLHANDLE handle, SQLSMALLINT completion);
SQLRETURN SQL_API swoole_odbc_SQLFreeHandle(SQLSMALLINT handle_type, SQLHANDLE handle);
SQLRETURN SQL_API swoole_odbc_SQLDisconnect(SQLHDBC dbc);
#ifdef SW_USE_ODBC_HOOK
#define SQLConnect swoole_odbc_SQLConnect
#define SQLDriverConnect swoole_odbc_SQLDriverConnect
#define SQLExecDirect swoole_odbc_SQLExecDirect
#define SQLGetInfo swoole_odbc_SQLGetInfo
#define SQLGetDiagRec swoole_odbc_SQLGetDiagRec
#define SQLPrepare swoole_odbc_SQLPrepare
#define SQLExecute swoole_odbc_SQLExecute
#define SQLCloseCursor swoole_odbc_SQLCloseCursor
#define SQLGetData swoole_odbc_SQLGetData
#define SQLPutData swoole_odbc_SQLPutData
#define SQLRowCount swoole_odbc_SQLRowCount
#define SQLDescribeCol swoole_odbc_SQLDescribeCol
#define SQLEndTran swoole_odbc_SQLEndTran
#define SQLFreeHandle swoole_odbc_SQLFreeHandle
#define SQLDisconnect swoole_odbc_SQLDisconnect
#endif
END_EXTERN_C()
#endif
#endif
