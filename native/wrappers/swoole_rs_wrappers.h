#ifndef SWOOLE_RS_WRAPPERS_H
#define SWOOLE_RS_WRAPPERS_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct { uint32_t accepted, wait_exit, initializers[3]; } SwrWrapperEventInit;
enum { SWR_WRAPPER_INIT_SOCKET = 1, SWR_WRAPPER_INIT_SYSTEM, SWR_WRAPPER_INIT_CLIENT };
/* No native pointer or state owner crosses Rust. Initializer IDs bind fixed native calls. */
SwrWrapperEventInit swr_v1_wrappers_event_init(uint32_t flags, uint32_t wait_exit_mask, uint32_t ready);
#ifdef __cplusplus
}
#endif
#endif
