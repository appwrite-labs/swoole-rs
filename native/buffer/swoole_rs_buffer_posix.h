#ifndef SWOOLE_RS_BUFFER_POSIX_H
#define SWOOLE_RS_BUFFER_POSIX_H
#include "swoole_rs_buffer.h"
#include <sys/uio.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct { size_t size, alignment, base_offset, length_offset; } swr_v1_buffer_iovec_abi;
/* Diagnostic binds the actual selected Rust target; no implementation credit. */
swr_v1_buffer_iovec_abi swr_v1_buffer_iovec_layout(void);
#ifdef __cplusplus
}
#endif
#endif
