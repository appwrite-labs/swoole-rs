#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* GlobalMemory is Rust's (crates/memory/src/members.rs); swoole_memory.h keeps its
 * constructor, which throws, and its inline members. new and alloc return 0, -1 for
 * Exception(SW_ERROR_INVALID_PARAMS) or -2 for std::bad_alloc. */
int32_t swr_v1_memory_global_memory_new(uint32_t pagesize, bool shared, void **output);
void swr_v1_memory_global_memory_delete(void *pool);
int32_t swr_v1_memory_global_memory_alloc(void **pool, uint32_t size, void **output);
#ifdef __cplusplus
}
#endif
