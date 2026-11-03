//! Upstream's `http2::Session` and `http2::Stream`: one session per HTTP/2
//! connection, keyed by its session id, holding its streams, settings,
//! windows and HPACK codecs. Each stream owns its request's
//! `swoole::http::Context`, created by C++ (`swr_h2s_stream_context`) and
//! released with the stream.
use crate::native::swoole_http2_client_coro_hpack_free;
use libc::c_void;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
use swoole_rs_http2::sw_rs_http2_default_settings;
use swoole_rs_zend::bailout;

/// What a session's handler gets: the stream's context, the session's
/// default context and the data it was created with.
#[repr(C)]
pub struct SessionRequest {
    pub ctx: *mut c_void,
    pub default_ctx: *mut c_void,
    pub data: *mut c_void,
}

/// `onRequest` for a finished request, C++ (a bailout is trapped around it).
pub type SessionHandler = unsafe extern "C" fn(*mut c_void);

unsafe extern "C" {
    /// A new context for stream `id`, a copy of `default_ctx`'s settings.
    fn swr_h2s_stream_context(default_ctx: *mut c_void, fd: i64, id: u32) -> *mut c_void;
    /// Suspend the current coroutine, published in `*slot` while it waits.
    fn swr_h2s_wait(slot: *mut *mut c_void);
    fn swr_h2s_wake(coroutine: *mut c_void);
}

pub(crate) type Session = Rc<SessionState>;
pub(crate) type Stream = Rc<StreamState>;

/// `http2::Session`.
pub(crate) struct SessionState {
    fd: i64,
    streams: RefCell<HashMap<u32, Stream>>,
    pub(crate) inflater: Cell<*mut c_void>,
    pub(crate) deflater: Cell<*mut c_void>,
    /// `local_settings`, in `swoole::http2::Settings` order; never changed.
    local: [u32; 6],
    /// `remote_settings`, as the peer's SETTINGS frames set them.
    pub(crate) remote: [Cell<u32>; 6],
    pub(crate) remote_window_size: Cell<u32>,
    pub(crate) local_window_size: Cell<u32>,
    pub(crate) max_body_size: u32,
    pub(crate) last_stream_id: Cell<u32>,
    pub(crate) shutting_down: Cell<bool>,
    pub(crate) is_coro: bool,
    /// The connection's context: frames that belong to no stream go out on it.
    pub(crate) default_ctx: *mut c_void,
    handle: SessionHandler,
    data: *mut c_void,
}

/// `http2::Stream`.
pub(crate) struct StreamState {
    pub(crate) ctx: *mut c_void,
    pub(crate) id: u32,
    pub(crate) remote_window_size: Cell<u32>,
    pub(crate) local_window_size: Cell<u32>,
    /// The coroutine waiting for the peer to open the window.
    waiting: Cell<*mut c_void>,
}

thread_local! {
    static SESSIONS: RefCell<HashMap<i64, Session>> = RefCell::new(HashMap::new());
}

fn defaults() -> [u32; 6] {
    let settings = sw_rs_http2_default_settings();
    [
        settings.header_table_size,
        settings.enable_push,
        settings.max_concurrent_streams,
        settings.init_window_size,
        settings.max_frame_size,
        settings.max_header_list_size,
    ]
}

/// `swoole::http2::Settings::init_window_size`.
const INIT_WINDOW_SIZE: usize = 3;

impl SessionState {
    fn new(
        fd: i64,
        default_ctx: *mut c_void,
        max_body_size: u32,
        is_coro: bool,
        handle: SessionHandler,
        data: *mut c_void,
    ) -> Self {
        // The peer need not send every setting: start from the defaults.
        let local = defaults();
        Self {
            fd,
            streams: RefCell::new(HashMap::new()),
            inflater: Cell::new(std::ptr::null_mut()),
            deflater: Cell::new(std::ptr::null_mut()),
            local,
            remote: local.map(Cell::new),
            remote_window_size: Cell::new(local[INIT_WINDOW_SIZE]),
            local_window_size: Cell::new(local[INIT_WINDOW_SIZE]),
            max_body_size,
            last_stream_id: Cell::new(0),
            shutting_down: Cell::new(false),
            is_coro,
            default_ctx,
            handle,
            data,
        }
    }

    /// The session of connection `fd`.
    pub(crate) fn find(fd: i64) -> Option<Session> {
        SESSIONS.with_borrow(|sessions| sessions.get(&fd).cloned())
    }

    /// The session a stream's context belongs to.
    pub(crate) fn of(ctx: *mut c_void) -> Option<Session> {
        // SAFETY: a live context.
        Self::find(unsafe { swoole_rs_phphttp::http2::fd(ctx) })
    }

    pub(crate) fn local(&self, field: usize) -> u32 {
        self.local[field]
    }

    /// `get_stream`, or `create_stream` when `create`.
    pub(crate) fn stream(&self, id: u32, create: bool) -> Option<Stream> {
        if !create {
            return self.streams.borrow().get(&id).cloned();
        }
        // SAFETY: the session's live default context.
        let ctx = unsafe { swr_h2s_stream_context(self.default_ctx, self.fd, id) };
        if ctx.is_null() {
            crate::server::log(
                SW_LOG_WARNING,
                SW_ERROR_HTTP2_STREAM_NO_HEADER,
                c"create_stream",
                &format!("http2 create stream#{id} context error"),
            );
            return None;
        }
        let stream = Rc::new(StreamState {
            ctx,
            id,
            remote_window_size: Cell::new(self.remote[INIT_WINDOW_SIZE].get()),
            local_window_size: Cell::new(self.local[INIT_WINDOW_SIZE]),
            waiting: Cell::new(std::ptr::null_mut()),
        });
        let replaced = self.streams.borrow_mut().insert(id, stream.clone());
        drop(replaced);
        Some(stream)
    }

    /// `remove_stream`: the stream is released once nothing holds it.
    pub(crate) fn remove(&self, id: u32) {
        let removed = self.streams.borrow_mut().remove(&id);
        drop(removed);
    }

    pub(crate) fn idle(&self) -> bool {
        self.streams.borrow().is_empty()
    }

    /// `handle(session, stream)`: onRequest; a bailout waits for `finish`.
    pub(crate) fn handle(&self, stream: &StreamState) {
        let mut request = SessionRequest {
            ctx: stream.ctx,
            default_ctx: self.default_ctx,
            data: self.data,
        };
        // SAFETY: C++'s handler, called with the request it expects.
        let result = unsafe { bailout::trap(self.handle, (&raw mut request).cast()) };
        bailout::defer(result);
    }
}

impl Drop for SessionState {
    fn drop(&mut self) {
        // SAFETY: the session's own codecs (or null).
        unsafe { swoole_http2_client_coro_hpack_free(self.inflater.get(), self.deflater.get()) };
    }
}

impl StreamState {
    /// Suspend the current coroutine until `wake`.
    pub(crate) fn wait(&self) {
        // SAFETY: in a coroutine; the slot lives as long as the stream, which
        // the caller holds.
        unsafe { swr_h2s_wait(self.waiting.as_ptr()) };
    }

    pub(crate) fn wake(&self) {
        let coroutine = self.waiting.get();
        if !coroutine.is_null() {
            // SAFETY: the coroutine suspended in `wait`.
            unsafe { swr_h2s_wake(coroutine) };
        }
    }
}

impl Drop for StreamState {
    fn drop(&mut self) {
        // SAFETY: the stream's own context, released once.
        unsafe { swoole_rs_phphttp::http2::release_stream(self.ctx) };
    }
}

/// `SW_LOG_WARNING` and `SW_ERROR_HTTP2_STREAM_NO_HEADER`.
const SW_LOG_WARNING: libc::c_int = 4;
const SW_ERROR_HTTP2_STREAM_NO_HEADER: libc::c_int = 3002;

/// Whether connection `fd` has a session.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phphttp2_session_exists(fd: i64) -> bool {
    SESSIONS.with_borrow(|sessions| sessions.contains_key(&fd))
}

/// A session for connection `fd` (`swoole_http2_server_session_new`), unless
/// it has one. `handle` gets each finished request with `data`.
/// # Safety
/// `default_ctx` is a live context that outlives the session; `handle` may be
/// called with a `SessionRequest`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_session_new(
    fd: i64,
    default_ctx: *mut c_void,
    max_body_size: u32,
    is_coro: bool,
    handle: SessionHandler,
    data: *mut c_void,
) {
    SESSIONS.with_borrow_mut(|sessions| {
        sessions.entry(fd).or_insert_with(|| {
            Rc::new(SessionState::new(
                fd,
                default_ctx,
                max_body_size,
                is_coro,
                handle,
                data,
            ))
        });
    });
}

/// `swoole_http2_server_session_free`: forget connection `fd`'s session; its
/// streams are released once nothing holds them.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phphttp2_session_free(fd: i64) {
    let session = SESSIONS.with_borrow_mut(|sessions| sessions.remove(&fd));
    drop(session);
}

/// Hold the stream of `ctx` (and so `ctx`) while C++ works on it across
/// coroutine switches; null if it has none.
/// # Safety
/// `ctx` is a live HTTP/2 context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_stream_hold(ctx: *mut c_void) -> *const c_void {
    // SAFETY: a live context.
    let id = unsafe { swoole_rs_phphttp::http2::stream_id(ctx) };
    SessionState::of(ctx)
        .and_then(|session| session.stream(id, false))
        .map_or(std::ptr::null(), |stream| Rc::into_raw(stream).cast())
}

/// # Safety
/// `stream` came from `swr_v1_phphttp2_stream_hold` (or is null), released once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_stream_release(stream: *const c_void) {
    if !stream.is_null() {
        // SAFETY: per the caller.
        drop(unsafe { Rc::from_raw(stream.cast::<StreamState>()) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_starts_from_the_default_settings() {
        unsafe extern "C" fn nothing(_: *mut c_void) {}
        let session = SessionState::new(
            7,
            std::ptr::null_mut(),
            0,
            false,
            nothing,
            std::ptr::null_mut(),
        );
        assert_eq!(session.local(INIT_WINDOW_SIZE), 65535);
        assert_eq!(session.remote[4].get(), 16384);
        assert_eq!(session.remote_window_size.get(), 65535);
        assert!(session.idle());
        assert!(!session.shutting_down.get());
        // Nothing to free: the codecs were never created.
        std::mem::forget(session);
    }
}
