// Generated from crates/corebase/src/switches.rs by tools/headers.sh; do not edit.
// The build's switches, which Rust reads (swoole_rs_corebase::built_with): many are
// the TLS library's header macros, which only a compile of its headers sees.
#include "swoole_ssl.h"

extern "C" const uint64_t swr_cpp_switches = 0
#if defined(SW_USE_THREAD_CONTEXT)
    | UINT64_C(1) << 0 /* ThreadContext */
#endif
#if defined(SW_USE_ASM_CONTEXT)
    | UINT64_C(1) << 1 /* AsmContext */
#endif
#if defined(SW_DEBUG)
    | UINT64_C(1) << 2 /* Debug */
#endif
#if defined(SW_LOG_TRACE_OPEN)
    | UINT64_C(1) << 3 /* Trace */
#endif
#if defined(HAVE_EPOLL)
    | UINT64_C(1) << 4 /* Epoll */
#endif
#if defined(HAVE_EVENTFD)
    | UINT64_C(1) << 5 /* Eventfd */
#endif
#if defined(SW_THREAD)
    | UINT64_C(1) << 6 /* Thread */
#endif
#if defined(HAVE_SIGNALFD)
    | UINT64_C(1) << 7 /* Signalfd */
#endif
#if defined(SW_USE_ACCEPT4)
    | UINT64_C(1) << 8 /* Accept4 */
#endif
#if defined(HAVE_CPU_AFFINITY)
    | UINT64_C(1) << 9 /* Affinity */
#endif
#if defined(HAVE_SPINLOCK)
    | UINT64_C(1) << 10 /* Spinlock */
#endif
#if defined(HAVE_RWLOCK)
    | UINT64_C(1) << 11 /* Rwlock */
#endif
#if defined(SW_SOCKETS)
    | UINT64_C(1) << 12 /* Sockets */
#endif
#if defined(SW_SUPPORT_DTLS)
    | UINT64_C(1) << 13 /* Dtls */
#endif
#if defined(SW_USE_CURL)
    | UINT64_C(1) << 14 /* Curl */
#endif
#if defined(SW_USE_CARES)
    | UINT64_C(1) << 15 /* Cares */
#endif
#if defined(SW_HAVE_ZLIB)
    | UINT64_C(1) << 16 /* Zlib */
#endif
#if defined(SW_HAVE_BROTLI)
    | UINT64_C(1) << 17 /* Brotli */
#endif
#if defined(SW_HAVE_ZSTD)
    | UINT64_C(1) << 18 /* Zstd */
#endif
#if defined(HAVE_MUTEX_TIMEDLOCK)
    | UINT64_C(1) << 19 /* Timedlock */
#endif
#if defined(HAVE_PTHREAD_BARRIER)
    | UINT64_C(1) << 20 /* Barrier */
#endif
#if defined(HAVE_FUTEX)
    | UINT64_C(1) << 21 /* Futex */
#endif
#if defined(SW_USE_MYSQLND)
    | UINT64_C(1) << 22 /* Mysqlnd */
#endif
#if defined(SW_USE_JEMALLOC)
    | UINT64_C(1) << 23 /* Jemalloc */
#endif
#if defined(SW_USE_TCMALLOC)
    | UINT64_C(1) << 24 /* Tcmalloc */
#endif
#if defined(SW_USE_PGSQL)
    | UINT64_C(1) << 25 /* Pgsql */
#endif
#if defined(SW_USE_ODBC)
    | UINT64_C(1) << 26 /* Odbc */
#endif
#if defined(SW_USE_ORACLE)
    | UINT64_C(1) << 27 /* Oracle */
#endif
#if defined(SW_USE_SQLITE)
    | UINT64_C(1) << 28 /* Sqlite */
#endif
#if defined(SW_USE_FIREBIRD)
    | UINT64_C(1) << 29 /* Firebird */
#endif
#if defined(SW_USE_IOURING)
    | UINT64_C(1) << 30 /* IoUring */
#endif
#if defined(SW_USE_URING_SOCKET)
    | UINT64_C(1) << 31 /* UringSocket */
#endif
#if defined(HAVE_BOOST_STACKTRACE)
    | UINT64_C(1) << 32 /* Stacktrace */
#endif
#if defined(HAVE_EXECINFO)
    | UINT64_C(1) << 33 /* Execinfo */
#endif
#if defined(SW_HAVE_SSH2LIB)
    | UINT64_C(1) << 34 /* Ssh2 */
#endif
#if defined(SW_HAVE_FTP)
    | UINT64_C(1) << 35 /* Ftp */
#endif
#if defined(HAVE_AI_V4MAPPED)
    | UINT64_C(1) << 36 /* AiV4mapped */
#endif
#if defined(HAVE_AI_ALL)
    | UINT64_C(1) << 37 /* AiAll */
#endif
#if defined(HAVE_AI_IDN)
    | UINT64_C(1) << 38 /* AiIdn */
#endif
#if defined(SSL_CTRL_SET_TLSEXT_HOSTNAME)
    | UINT64_C(1) << 39 /* TlsHostName */
#endif
#if defined(OPENSSL_IS_BORINGSSL)
    | UINT64_C(1) << 40 /* BoringSsl */
#endif
#if defined(HAVE_IOURING_STATX)
    | UINT64_C(1) << 41 /* IouringStatx */
#endif
#if defined(HAVE_IOURING_FTRUNCATE)
    | UINT64_C(1) << 42 /* IouringFtruncate */
#endif
#if defined(HAVE_IOURING_FUTEX)
    | UINT64_C(1) << 43 /* IouringFutex */
#endif
#if defined(TLS1_1_VERSION)
    | UINT64_C(1) << 44 /* Tls1_1 */
#endif
#if defined(TLS1_2_VERSION)
    | UINT64_C(1) << 45 /* Tls1_2 */
#endif
#if defined(TLS1_3_VERSION)
    | UINT64_C(1) << 46 /* Tls1_3 */
#endif
#if defined(HAVE_SSL3)
    | UINT64_C(1) << 47 /* Ssl3 */
#endif
#if defined(SW_CONTEXT_PROTECT_STACK_PAGE)
    | UINT64_C(1) << 48 /* ContextProtectStackPage */
#endif
#if defined(SW_CONTEXT_DETECT_STACK_USAGE)
    | UINT64_C(1) << 49 /* ContextDetectStackUsage */
#endif
#if defined(SW_CORO_TIME)
    | UINT64_C(1) << 50 /* CoroTime */
#endif
    ;

// The bug report's compile-time lines (swoole_rs_corebase::bug_report_init).
extern "C" const char *const swr_cpp_bug_report[3] = {
    SWOOLE_BUG_REPORT, __VERSION__, "OPENSSL_VERSION: " OPENSSL_VERSION_TEXT "\n"};
