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

/* The coroutine plain-files wrapper's url_stat, unlink, rename, mkdir and rmdir
 * (crates/phpruntime's plain_files), which plain_wrapper.c's wrapper table names. */
struct _php_stream_wrapper;
struct _php_stream_statbuf;
struct _php_stream_context;
int swr_v1_phpruntime_plain_url_stat(struct _php_stream_wrapper *wrapper, const char *url, int flags,
                                     struct _php_stream_statbuf *ssb, struct _php_stream_context *context);
int swr_v1_phpruntime_plain_unlink(struct _php_stream_wrapper *wrapper, const char *url, int options,
                                   struct _php_stream_context *context);
int swr_v1_phpruntime_plain_rename(struct _php_stream_wrapper *wrapper, const char *from, const char *to, int options,
                                   struct _php_stream_context *context);
int swr_v1_phpruntime_plain_mkdir(struct _php_stream_wrapper *wrapper, const char *dir, int mode, int options,
                                  struct _php_stream_context *context);
/* php_stream_parse_fopen_modes and php_stream_mode_sanitize_fdopen_fopencookie: an fopen mode's
 * open(2) flags (false for no mode), and a stream's mode as fdopen takes it, in fixed[5]. */
bool swr_v1_phpruntime_plain_open_flags(const char *mode, int *flags);
void swr_v1_phpruntime_plain_fdopen_mode(const char *mode, uint8_t *fixed);
struct utimbuf;
int swr_v1_phpruntime_plain_metadata(const char *url, int option, const struct utimbuf *times, const char *name,
                                     long number);
int swr_v1_phpruntime_plain_rmdir(struct _php_stream_wrapper *wrapper, const char *url, int options,
                                  struct _php_stream_context *context);

/* Defined by thirdparty/php/streams/plain_wrapper.c, which keeps PHP's originals:
 * RINIT, and the stream_ops entry. */
void php_swoole_runtime_rinit(void);
void php_swoole_runtime_stream_ops(uint8_t stdio, uint8_t hook);

#ifdef __cplusplus
}
#endif
#endif
