#ifndef SWOOLE_RS_BUFFER_H
#define SWOOLE_RS_BUFFER_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* The Rust queue behind swoole::Buffer (crates/buffer). Only Rust calls its operations. */
typedef struct swr_v1_buffer swr_v1_buffer;
#ifdef __cplusplus
}
#endif
#endif
