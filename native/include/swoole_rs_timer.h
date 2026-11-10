#ifndef SWOOLE_RS_TIMER_H
#define SWOOLE_RS_TIMER_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

/* crates/timer's queue, the reads native/timer/swoole_timer.h makes of it
 * (docs/timer-abi-v1.md). Null queues and outputs answer SWR_TIMER_INVALID. */
typedef struct swr_v1_timer swr_v1_timer;
enum { SWR_TIMER_OK = 0, SWR_TIMER_INVALID = -1, SWR_TIMER_OVERFLOW = -2, SWR_TIMER_CAPACITY = -7 };
typedef struct {
    size_t count;
    uint64_t round;
    int64_t active_id;
    uint32_t selecting, closing;
} swr_v1_timer_stats;

/* The token id was added with, or null. */
void *swr_v1_timer_token(const swr_v1_timer *queue, int64_t id);
int32_t swr_v1_timer_stats_get(const swr_v1_timer *queue, swr_v1_timer_stats *out);
/* Query count with ids=NULL/capacity=0; CAPACITY writes count, never partial IDs. */
int32_t swr_v1_timer_ids(const swr_v1_timer *queue, int64_t *ids, size_t capacity, size_t *count);

#ifdef __cplusplus
}
#endif
#endif
