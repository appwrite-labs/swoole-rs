# Status message fallback buffer (`swr_v2_http_status_message`)

`get_status_message` forwards to Rust. Known messages are the existing immutable
strings. Unknown codes are formatted into a const-initialized Rust thread-local
`UnsafeCell<[u8; 128]>`, which replaces the old native thread-local array. Rust
returns a raw pointer after all borrows end. The public C++ signature is unchanged.

Lifetime: unknown calls on the same thread reuse the same buffer, and known calls
leave it untouched. Each thread has its own buffer. A returned pointer stays valid
until the thread exits and is overwritten by that thread's next unknown call. Do
not share it across threads.

Tests: `crates/http/src/pin_helpers.rs` (`status_messages_and_errno_match_the_pin`,
`unknown_status_fallback_is_per_thread`). The public
status-code array is covered in [status-codes.md](status-codes.md).
