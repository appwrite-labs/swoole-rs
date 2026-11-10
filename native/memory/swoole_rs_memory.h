#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <pthread.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* swoole::Channel: the queue state, its process-shared mutex and its notification
 * pipe (read end first). Native allocates swr_v1_memory_channel_bytes() bytes (heap
 * or shared mapping), and frees them after swr_v1_memory_channel_close(); the queue
 * storage follows the record. */
typedef struct swr_v1_memory_channel {
    int64_t head, tail;
    size_t size;
    uint8_t head_tag, tail_tag;
    int32_t num, max_num;
    size_t bytes;
    int32_t flags, maxlen;
    void *mem;
    pthread_mutex_t lock;
    int notify[2];
} swr_v1_memory_channel;
/* 0 for an invalid or unrepresentable configuration. */
size_t swr_v1_memory_channel_bytes(size_t size, size_t maxlen);
void swr_v1_memory_channel_close(swr_v1_memory_channel *);
/* GlobalMemory is Rust's (crates/memory/src/members.rs); swoole_memory.h keeps its
 * constructor, which throws, and its inline members. new and alloc return 0, -1 for
 * Exception(SW_ERROR_INVALID_PARAMS) or -2 for std::bad_alloc. */
int32_t swr_v1_memory_global_memory_new(uint32_t pagesize, bool shared, void **output);
void swr_v1_memory_global_memory_delete(void *pool);
int32_t swr_v1_memory_global_memory_alloc(void **pool, uint32_t size, void **output);
#ifdef __cplusplus
}
#endif
