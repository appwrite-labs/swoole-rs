/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_buffer.h, owned here: the output queue whose members
 * crates/buffer/src/cpp.rs defines under their upstream symbols. Upstream's file is
 * deleted, so `#include "swoole_buffer.h"` finds this copy on the Rust prefix's include path. */
#pragma once

#include <queue>
#include "swoole_rs_buffer.h"
#include "swoole_rs_buffer_typed.h"
#include <sys/uio.h>

namespace swoole {

// The kinds of a queued entry, as the typed queue numbers them.
struct BufferChunk {
    enum Type {
        TYPE_DATA,
        TYPE_SENDFILE,
        TYPE_CLOSE,
    };
};

// Its only member is the Rust queue handle.
class Buffer {
    swr_v1_buffer *rs_buffer_;

  public:
    explicit Buffer(uint32_t _chunk_size);
    ~Buffer();

    using TypedFront = swr_v1_buffer_typed_front_result;
    TypedFront typed_front() const;
    void typed_consume(const TypedFront &front, uint32_t count);
    void typed_close();

    void pop();
    void append(const char *data, uint32_t size);
    void append(const struct iovec *iov, size_t iovcnt, off_t offset);

    uint32_t length() const;
    size_t count() const;
    bool empty() const;

    static bool empty(const Buffer *buffer) {
        return buffer == nullptr || buffer->empty();
    }
};

}  // namespace swoole
