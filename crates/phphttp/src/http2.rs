//! What the HTTP/2 server (crates/phphttp2) reads and does to its streams'
//! contexts.
use crate::{context::swr_v1_phphttp_context_free, host::Context, send};

/// The connection's session id.
/// # Safety
/// `ctx` is a live context.
#[must_use]
pub unsafe fn fd(ctx: *mut Context) -> i64 {
    send::state(ctx).fd
}

/// The context's HTTP/2 stream, 0 once it is released.
/// # Safety
/// `ctx` is a live context.
#[must_use]
pub unsafe fn stream_id(ctx: *mut Context) -> u32 {
    send::state(ctx).stream_id
}

/// `~Stream`: the stream lets go of its context, which is complete.
/// # Safety
/// `ctx` is a live stream context, released once.
pub unsafe fn release_stream(ctx: *mut Context) {
    let state = send::state(ctx);
    state.stream_id = 0;
    state.end = 1;
    // SAFETY: per the caller.
    unsafe { swr_v1_phphttp_context_free(ctx) };
}
