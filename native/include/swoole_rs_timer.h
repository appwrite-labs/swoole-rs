#ifndef SWOOLE_RS_TIMER_H
#define SWOOLE_RS_TIMER_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

typedef struct swr_v1_timer swr_v1_timer;
enum {
    SWR_TIMER_OK = 0, SWR_TIMER_EMPTY = 1, SWR_TIMER_DEFERRED = 2,
    SWR_TIMER_RELEASE = 3, SWR_TIMER_RELEASE_OVERFLOW = 4,
    SWR_TIMER_INVALID = -1, SWR_TIMER_OVERFLOW = -2, SWR_TIMER_NOMEM = -3,
    SWR_TIMER_BUSY = -4, SWR_TIMER_NOT_FOUND = -5, SWR_TIMER_CLOSING = -6,
    SWR_TIMER_CAPACITY = -7, SWR_TIMER_PENDING = -8
};
typedef struct {
    int64_t id, deadline, interval;
    uint64_t exec_count, birth_round;
    uint32_t removed, reserved;
    void *token;
} swr_v1_timer_view;
typedef struct {
    size_t count;
    uint64_t round;
    int64_t active_id;
    uint32_t selecting, closing;
} swr_v1_timer_stats;

/* Thread-confined queue; never calls native code or dereferences token. */
int32_t swr_v1_timer_create(swr_v1_timer **out);
int32_t swr_v1_timer_release(swr_v1_timer *queue);
int32_t swr_v1_timer_add(swr_v1_timer *queue, int64_t now, int64_t delay,
                        uint32_t persistent, void *token, swr_v1_timer_view *out);
int32_t swr_v1_timer_get(const swr_v1_timer *queue, int64_t id, swr_v1_timer_view *out);
/* The token id was added with, or null. */
void *swr_v1_timer_token(const swr_v1_timer *queue, int64_t id);
int32_t swr_v1_timer_update(swr_v1_timer *queue, int64_t id, int64_t deadline,
                           swr_v1_timer_view *out);
/* RELEASE transfers token; DEFERRED retains the active removed entry. */
int32_t swr_v1_timer_remove(swr_v1_timer *queue, int64_t id, swr_v1_timer_view *out);
int32_t swr_v1_timer_begin(swr_v1_timer *queue, int64_t now);
int32_t swr_v1_timer_next(swr_v1_timer *queue, swr_v1_timer_view *out);
/* OK retains a repeat; RELEASE[_OVERFLOW] transfers the exact active token. */
int32_t swr_v1_timer_complete(swr_v1_timer *queue, int64_t id, swr_v1_timer_view *out);
int32_t swr_v1_timer_end(swr_v1_timer *queue, int64_t *next_delay);
int32_t swr_v1_timer_stats_get(const swr_v1_timer *queue, swr_v1_timer_stats *out);
/* Query count with ids=NULL/capacity=0; CAPACITY writes count, never partial IDs. */
int32_t swr_v1_timer_ids(const swr_v1_timer *queue, int64_t *ids, size_t capacity, size_t *count);
int32_t swr_v1_timer_close(swr_v1_timer *queue);
int32_t swr_v1_timer_drain(swr_v1_timer *queue, swr_v1_timer_view *out);

#ifdef __cplusplus
}
#endif
#endif
