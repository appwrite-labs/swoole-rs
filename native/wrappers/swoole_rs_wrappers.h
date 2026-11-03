#ifndef SWOOLE_RS_WRAPPERS_H
#define SWOOLE_RS_WRAPPERS_H
#ifndef SWOOLE_RS_WRAPPERS
#error "Wrapper overlay requires --enable-swoole-rs-wrappers"
#endif
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct SwrWrapperRequest SwrWrapperRequest;
typedef struct { size_t separator; uint32_t valid, default_host; } SwrWrapperListenerPlan;
/* Borrowed immutable initialized address bytes, valid for this call; no retained pointer.
 * First colon only; native strings, strtol, server and callback effects stay native. */
SwrWrapperListenerPlan swr_v1_wrappers_listener_plan(const uint8_t *, size_t);
enum SwrWrapperResponseAction {
    SWR_WRAPPER_RESPONSE_START = 0,
    SWR_WRAPPER_RESPONSE_HEADER = 1,
    SWR_WRAPPER_RESPONSE_BODY = 2,
    SWR_WRAPPER_RESPONSE_OBSERVE = 3,
    SWR_WRAPPER_RESPONSE_CLOSE = 4,
    SWR_WRAPPER_RESPONSE_DONE = 5,
    SWR_WRAPPER_RESPONSE_FAILED = 6
};
/* Stack facts only. Return before every send, warning, field read or close.
 * keepalive is observed natively only for OBSERVE, after successful sends. */
uint32_t swr_v1_wrappers_response_next(uint32_t previous, size_t length, uint32_t succeeded, uint32_t keepalive);
typedef struct { uint32_t accepted, wait_exit, initializers[3]; } SwrWrapperEventInit;
enum { SWR_WRAPPER_INIT_SOCKET = 1, SWR_WRAPPER_INIT_SYSTEM, SWR_WRAPPER_INIT_CLIENT };
/* No native pointer or state owner crosses Rust. Initializer IDs bind fixed native calls. */
SwrWrapperEventInit swr_v1_wrappers_event_init(uint32_t flags, uint32_t wait_exit_mask, uint32_t ready);
/* Decimal bytes into live writable caller storage; no NUL, allocation or retained borrow.
 * SIZE_MAX means insufficient capacity/null output with no publication. */
size_t swr_v1_wrappers_content_length(size_t length, uint8_t *out, size_t capacity);
typedef struct { const uint8_t *data; size_t len; } SwrWrapperSpan;
typedef struct { SwrWrapperSpan header, form, input, body; uint32_t flags; } SwrWrapperView;
SwrWrapperRequest *swr_v1_wrappers_request_new(void);
void swr_v1_wrappers_request_free(SwrWrapperRequest *);
int32_t swr_v1_wrappers_request_view(const SwrWrapperRequest *, SwrWrapperView *);
int32_t swr_v1_wrappers_header(SwrWrapperRequest *, const uint8_t *, size_t);
int32_t swr_v1_wrappers_header_kind(const SwrWrapperRequest *, uint32_t method, const uint8_t *, size_t, uint8_t multipart);
void swr_v1_wrappers_disposition_begin(SwrWrapperRequest *);
int32_t swr_v1_wrappers_parameter(SwrWrapperRequest *, const uint8_t *, size_t, const uint8_t *, size_t);
int32_t swr_v1_wrappers_disposition_end(SwrWrapperRequest *);
/* Initialized immutable disposition bytes, disjoint from request-owned buffers.
 * No pointer retained; host-libc whitespace, first NUL, last duplicate and
 * filename-presence precedence. Publishes only selected form/input metadata. */
int32_t swr_v1_wrappers_disposition(SwrWrapperRequest *, const uint8_t *, size_t);
size_t swr_v1_wrappers_body_skip(SwrWrapperRequest *, const uint8_t *, size_t);
int32_t swr_v1_wrappers_body_append(SwrWrapperRequest *, const uint8_t *, size_t);
void swr_v1_wrappers_part_end(SwrWrapperRequest *);
void swr_v1_wrappers_complete(SwrWrapperRequest *);
size_t swr_v1_wrappers_split(const uint8_t *, size_t, uint8_t delimiter);
size_t swr_v1_wrappers_header_wire(const uint8_t *, size_t, const uint8_t *, size_t, uint8_t *, size_t);
/* http_server::listen's llhttp and multipart callbacks (crates/wrappers/src/http.rs) run
 * over the request bound to a Context; each effect on it goes through `native`. */
enum {
    SWR_WRAPPER_HTTP_PATH = 0, SWR_WRAPPER_HTTP_QUERY, SWR_WRAPPER_HTTP_HEADER, SWR_WRAPPER_HTTP_FORM_URLENCODED,
    SWR_WRAPPER_HTTP_HEADERS_COMPLETE, SWR_WRAPPER_HTTP_BODY, SWR_WRAPPER_HTTP_MULTIPART_NEW,
    SWR_WRAPPER_HTTP_MULTIPART_FREE, SWR_WRAPPER_HTTP_MULTIPART_EXECUTE, SWR_WRAPPER_HTTP_LOG_PARSE,
    SWR_WRAPPER_HTTP_FILE, SWR_WRAPPER_HTTP_HAS_FILE, SWR_WRAPPER_HTTP_WRITE, SWR_WRAPPER_HTTP_OPEN,
    SWR_WRAPPER_HTTP_FORM_DATA, SWR_WRAPPER_HTTP_FCLOSE, SWR_WRAPPER_HTTP_BOUNDARY
};
typedef uint64_t (*SwrWrapperNative)(void *context, uint32_t op, void *object, const uint8_t *a, size_t an,
                                     const uint8_t *b, size_t bn);
void swr_v1_wrappers_request_bind(SwrWrapperRequest *, void *context, SwrWrapperNative native);
/* The llhttp_settings_t of those callbacks. */
const void *swr_v1_wrappers_http_settings(void);
uint8_t swr_v1_wrappers_event_wait(uint8_t wait_exit, uint8_t exit);
int32_t swr_v1_wrappers_event_direction(int32_t event);
#ifdef __cplusplus
}
#endif
#endif
