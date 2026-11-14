#ifndef SWOOLE_RS_ERROR_RESOLVER_H
#define SWOOLE_RS_ERROR_RESOLVER_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Optional formatter: no throw, Zend/PHP calls, bailout, or retained Rust state.
 * Called synchronously only for unknown codes. Returned bytes remain native-owned.
 * A null formatter returns null on misses. Known messages are immortal and immutable. */
typedef const char *(*swr_v1_error_formatter)(int32_t code);
const char *swr_v1_error_resolve(int32_t code, swr_v1_error_formatter fallback);
/* Full native-compatible dispatcher. System callback only for codes <500.
 * Unknown codes use one mutable 32-byte Rust buffer: serialize their calls/use.
 * Known codes retain immortal immutable storage. No allocation or unwind. */
const char *swr_v1_error_strerror(int32_t code, swr_v1_error_formatter system);
#ifdef __cplusplus
}
#endif
#endif
