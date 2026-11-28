#ifndef SWOOLE_RS_ERROR_STATE_H
#define SWOOLE_RS_ERROR_STATE_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct SwrErrorIgnored SwrErrorIgnored;
/* Process-local, externally serialized; null means empty. No callback/unwind. */
/* Insert: 0 success, -1 invalid handle slot, -2 allocation failure. */
int32_t swr_v1_error_ignore(SwrErrorIgnored **state, int32_t code);
int32_t swr_v1_error_is_ignored(const SwrErrorIgnored *state, int32_t code);
void swr_v1_error_ignored_release(SwrErrorIgnored *state);
#ifdef __cplusplus
}
#endif
#endif
