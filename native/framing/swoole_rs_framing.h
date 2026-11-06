#ifndef SWOOLE_RS_FRAMING_H
#define SWOOLE_RS_FRAMING_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* One receive (crates/framing/src/recv.rs): Rust reads and writes the swoole::String
 * head and the Protocol's leading fields in place; `call` applies the WAIT flag to
 * socket->recv_wait, performs one effect and refreshes `flags`; the caller applies
 * WAIT once more after return. */
enum { SWR_FRAMING_MODE_LENGTH, SWR_FRAMING_MODE_EOF, SWR_FRAMING_MODE_SPLIT };
enum {
    SWR_FRAMING_OP_RECV = 1, SWR_FRAMING_OP_GET_LENGTH, SWR_FRAMING_OP_EXTEND, SWR_FRAMING_OP_DISPATCH, SWR_FRAMING_OP_REDUCE, SWR_FRAMING_OP_SPLIT = 7,
    SWR_FRAMING_OP_SSL_PENDING, SWR_FRAMING_OP_CLEAR_SKIP, SWR_FRAMING_OP_PAGE, SWR_FRAMING_OP_LOG_MALFORMED,
    SWR_FRAMING_OP_LOG_NO_LENGTH, SWR_FRAMING_OP_LOG_TOO_LARGE
};
enum { SWR_FRAMING_REMOVED = 1, SWR_FRAMING_WAIT = 2, SWR_FRAMING_TLS = 4, SWR_FRAMING_SKIP = 8, SWR_FRAMING_DYNAMIC = 16 };
#define SWR_FRAMING_RECV_CLOSE (-1)
#define SWR_FRAMING_RECV_DONE (-2)
#define SWR_FRAMING_THROWN INT64_MIN
typedef struct { const void *protocol; void *buffer; uint32_t mode, head, flags; } SwrFramingIo;
typedef int64_t (*SwrFramingCall)(void *context, uint32_t op, uint64_t arg);
int32_t swr_v1_framing_recv(SwrFramingIo *, void *context, SwrFramingCall);
size_t swr_v1_framing_width(uint8_t);
int32_t swr_v1_framing_unpack(uint8_t, const uint8_t *, size_t, int64_t *);
uint64_t swr_v1_framing_hton64(uint64_t);
int32_t swr_v1_framing_default(uint8_t, const uint8_t *, size_t, uint32_t offset, uint32_t size, uint32_t body_offset, int64_t *);
/* mime_type::get and mime_type::exists over a borrowed filename (crates/framing/src/mime.rs).
 * get's text stays valid until PHP sets or deletes that suffix. */
const char *swr_v1_framing_mime_get(const uint8_t *, size_t);
bool swr_v1_framing_mime_exists(const uint8_t *, size_t);
#ifdef __cplusplus
}
#endif
#endif
