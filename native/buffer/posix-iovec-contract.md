# POSIX iovec append (`swr_v1_buffer_append_iovec`)

`swr_v1_buffer_append_iovec` accepts the target's real C `struct iovec` array
(through the pinned `libc` 0.2.191 binding) and a signed byte offset. It shares
`swr_v1_buffer_append`'s descriptor traversal, validation, chunk allocation/copy,
accounting and transactional rollback.

The native `append(iovec)` body forwards the descriptor array directly and
translates the status after Rust returns. There is no native staging loop and no
descriptor allocation. Rust rejects a negative offset, a null non-empty array, an
unrepresentable count, invalid byte spans and overflowing sums before any mutation.
Queue and data allocations remain fallible.

Descriptors and bytes must stay valid, stable and non-aliased for the duration of
the call. Nothing is retained.

`swr_v1_buffer_iovec_layout` reports the binding's sizeof/alignof/offsetof so a C++
caller can compare them. On macOS arm64 and Linux/glibc aarch64, size is 16,
alignment 8, base offset 0 and length offset 8. Other platforms have not been
tested.

Test: `python3 -m pytest tests/modules/buffer/test_iovec.py --archive ARCHIVE --sanitize`.
