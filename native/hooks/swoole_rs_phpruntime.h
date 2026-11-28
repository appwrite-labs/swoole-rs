/* Swoole\Runtime (Rust, crates/phpruntime): the class and SWOOLE_HOOK_*
 * constants, the hook flags, PHPCoroutine::enable_hook's family transitions,
 * the function-table hooks and the sleep family's replacements (their
 * zif_swoole_* handlers), its methods and the hooked stream functions; each
 * raises a trapped bailout last. plain_wrapper.c keeps PHP's stream wrappers. */
#ifndef SWOOLE_RS_PHPRUNTIME_H
#define SWOOLE_RS_PHPRUNTIME_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/types.h>
#include <sys/uio.h>
#ifdef __cplusplus
extern "C" {
#endif
/* RINIT: 1 on the main thread (the plain wrapper then saves the original stream ops). */
uint8_t swr_v1_phpruntime_rinit(void);
/* The hook flags that select C++'s PDO drivers and native curl (crates/phpruntime's flags). */
enum {
    SWR_HOOK_NATIVE_CURL = 1u << 12,
    SWR_HOOK_PDO_PGSQL = 1u << 16,
    SWR_HOOK_PDO_ODBC = 1u << 17,
    SWR_HOOK_PDO_ORACLE = 1u << 18,
    SWR_HOOK_PDO_SQLITE = 1u << 19,
    SWR_HOOK_PDO_FIREBIRD = 1u << 20,
};

/* The coroutine socket streams: the "socket/coroutine" operations and
 * php_swoole_create_stream_from_socket. */
struct _php_stream;
struct _php_stream *swr_v1_phpruntime_stream_from_socket(int fd, int domain, int kind, int protocol);

/* Defined by thirdparty/php/streams/plain_wrapper.c, which keeps PHP's originals:
 * RINIT, and the stream_ops entry. */
void php_swoole_runtime_rinit(void);
void php_swoole_runtime_stream_ops(uint8_t stdio, uint8_t hook);

#ifdef __cplusplus
}
#endif
#endif
