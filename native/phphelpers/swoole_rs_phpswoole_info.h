/* Generated from phpswoole's capability inventory; do not edit. */
#ifndef SWOOLE_RS_PHPSWOOLE_INFO_H
#define SWOOLE_RS_PHPSWOOLE_INFO_H
#include <stdint.h>
static uint64_t swr_cpp_phpswoole_info_flags() {
    uint64_t flags = 0;
#if defined(SW_USE_THREAD_CONTEXT)
    flags |= UINT64_C(1) << 0; /* "" */
#endif
#if defined(SW_USE_ASM_CONTEXT)
    flags |= UINT64_C(1) << 1; /* "" */
#endif
#if defined(SW_DEBUG)
    flags |= UINT64_C(1) << 2; /* "debug" */
#endif
#if defined(SW_LOG_TRACE_OPEN)
    flags |= UINT64_C(1) << 3; /* "trace_log" */
#endif
#if defined(HAVE_EPOLL)
    flags |= UINT64_C(1) << 4; /* "epoll" */
#endif
#if defined(HAVE_EVENTFD)
    flags |= UINT64_C(1) << 5; /* "eventfd" */
#endif
#if defined(SW_THREAD)
    flags |= UINT64_C(1) << 6; /* "thread" */
#endif
#if defined(HAVE_SIGNALFD)
    flags |= UINT64_C(1) << 7; /* "signalfd" */
#endif
#if defined(SW_USE_ACCEPT4)
    flags |= UINT64_C(1) << 8; /* "accept4" */
#endif
#if defined(HAVE_CPU_AFFINITY)
    flags |= UINT64_C(1) << 9; /* "cpu_affinity" */
#endif
#if defined(HAVE_SPINLOCK)
    flags |= UINT64_C(1) << 10; /* "spinlock" */
#endif
#if defined(HAVE_RWLOCK)
    flags |= UINT64_C(1) << 11; /* "rwlock" */
#endif
#if defined(SW_SOCKETS)
    flags |= UINT64_C(1) << 12; /* "sockets" */
#endif
#if defined(SW_SUPPORT_DTLS)
    flags |= UINT64_C(1) << 13; /* "dtls" */
#endif
#if defined(SW_USE_CURL)
    flags |= UINT64_C(1) << 14; /* "curl-native" */
#endif
#if defined(SW_USE_CARES)
    flags |= UINT64_C(1) << 15; /* "" */
#endif
#if defined(SW_HAVE_ZLIB)
    flags |= UINT64_C(1) << 16; /* "" */
#endif
#if defined(SW_HAVE_BROTLI)
    flags |= UINT64_C(1) << 17; /* "" */
#endif
#if defined(SW_HAVE_ZSTD)
    flags |= UINT64_C(1) << 18; /* "" */
#endif
#if defined(HAVE_MUTEX_TIMEDLOCK)
    flags |= UINT64_C(1) << 19; /* "mutex_timedlock" */
#endif
#if defined(HAVE_PTHREAD_BARRIER)
    flags |= UINT64_C(1) << 20; /* "pthread_barrier" */
#endif
#if defined(HAVE_FUTEX)
    flags |= UINT64_C(1) << 21; /* "futex" */
#endif
#if defined(SW_USE_MYSQLND)
    flags |= UINT64_C(1) << 22; /* "mysqlnd" */
#endif
#if defined(SW_USE_JEMALLOC)
    flags |= UINT64_C(1) << 23; /* "jemalloc" */
#endif
#if defined(SW_USE_TCMALLOC)
    flags |= UINT64_C(1) << 24; /* "tcmalloc" */
#endif
#if defined(SW_USE_PGSQL)
    flags |= UINT64_C(1) << 25; /* "" */
#endif
#if defined(SW_USE_ODBC)
    flags |= UINT64_C(1) << 26; /* "coroutine_odbc" */
#endif
#if defined(SW_USE_ORACLE)
    flags |= UINT64_C(1) << 27; /* "coroutine_oracle" */
#endif
#if defined(SW_USE_SQLITE)
    flags |= UINT64_C(1) << 28; /* "coroutine_sqlite" */
#endif
#if defined(SW_USE_FIREBIRD)
    flags |= UINT64_C(1) << 29; /* "coroutine_firebird" */
#endif
#if defined(SW_USE_IOURING)
    flags |= UINT64_C(1) << 30; /* "io_uring" */
#endif
#if defined(SW_USE_URING_SOCKET)
    flags |= UINT64_C(1) << 31; /* "uring_socket" */
#endif
#if defined(HAVE_BOOST_STACKTRACE)
    flags |= UINT64_C(1) << 32; /* "boost stacktrace" */
#endif
#if defined(HAVE_EXECINFO)
    flags |= UINT64_C(1) << 33; /* "execinfo" */
#endif
#if defined(SW_HAVE_SSH2LIB)
    flags |= UINT64_C(1) << 34; /* "" */
#endif
#if defined(SW_HAVE_FTP)
    flags |= UINT64_C(1) << 35; /* "" */
#endif
#if defined(HAVE_AI_V4MAPPED)
    flags |= UINT64_C(1) << 36; /* "" */
#endif
#if defined(HAVE_AI_ALL)
    flags |= UINT64_C(1) << 37; /* "" */
#endif
#if defined(HAVE_AI_IDN)
    flags |= UINT64_C(1) << 38; /* "" */
#endif
#if defined(SSL_CTRL_SET_TLSEXT_HOSTNAME)
    flags |= UINT64_C(1) << 39; /* "" */
#endif
#if defined(OPENSSL_IS_BORINGSSL)
    flags |= UINT64_C(1) << 40; /* "" */
#endif
    return flags;
}
#endif
