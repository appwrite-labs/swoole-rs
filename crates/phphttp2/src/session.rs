//! Upstream's `http2::Session` and `http2::Stream`: one session per HTTP/2
//! connection, keyed by its session id, holding its streams, settings,
//! windows and HPACK codecs. Each stream owns its request's
//! `swoole::http::Context`, a copy of the session's settings context, released
//! with the stream.
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    hash::{BuildHasherDefault, Hasher},
    rc::Rc,
};
use swoole_rs_http2::{
    hpack::{Decoder, Encoder},
    sw_rs_http2_default_settings,
};
use swoole_rs_phphttp::http2::{Ctx, Handler, ServerCtx, StreamCtx};
use swoole_rs_zend::bailout;

pub(crate) type Session = Rc<SessionState>;

/// Session fds and stream ids are small native counters, never peer-chosen
/// hash keys: hash them by a multiply instead of `SipHash`.
#[derive(Default)]
pub(crate) struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0.wrapping_mul(0x9e37_79b9_7f4a_7c15)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(byte);
        }
    }

    fn write_u32(&mut self, id: u32) {
        self.0 = u64::from(id);
    }

    fn write_i64(&mut self, fd: i64) {
        self.0 = fd.cast_unsigned();
    }
}

pub(crate) type IdMap<K, V> = HashMap<K, V, BuildHasherDefault<IdHasher>>;
pub(crate) type Stream = Rc<StreamState>;

/// `http2::Session`.
pub(crate) struct SessionState {
    fd: i64,
    streams: RefCell<IdMap<u32, Stream>>,
    pub(crate) inflater: RefCell<Decoder>,
    /// Made on first use, sized by the peer's `SETTINGS_HEADER_TABLE_SIZE` then.
    pub(crate) deflater: RefCell<Option<Encoder>>,
    /// `local_settings`, in `swoole::http2::Settings` order; never changed.
    local: [u32; 6],
    /// `remote_settings`, as the peer's SETTINGS frames set them.
    pub(crate) remote: [Cell<u32>; 6],
    pub(crate) remote_window_size: Cell<u32>,
    pub(crate) local_window_size: Cell<u32>,
    pub(crate) max_body_size: u32,
    pub(crate) last_stream_id: Cell<u32>,
    /// The stream whose header block awaits CONTINUATION frames (0: none),
    /// and whether that block's HEADERS frame ended the stream.
    pub(crate) header_stream_id: Cell<u32>,
    pub(crate) header_end_stream: Cell<bool>,
    pub(crate) shutting_down: Cell<bool>,
    /// The connection's context: frames that belong to no stream go out on it.
    pub(crate) default_ctx: Ctx,
    /// An async server's settings context, made for this session alone (a
    /// coroutine server's is its connection's).
    own: Option<ServerCtx>,
    handle: Handler,
}

/// `http2::Stream`.
pub(crate) struct StreamState {
    pub(crate) ctx: Ctx,
    /// The stream's hold on `ctx`, let go with the stream.
    hold: StreamCtx,
    pub(crate) id: u32,
    pub(crate) remote_window_size: Cell<u32>,
    pub(crate) local_window_size: Cell<u32>,
    /// The request's header block is complete; later blocks are trailers.
    pub(crate) headers_received: Cell<bool>,
    /// The request is complete: no later frame adds to it.
    pub(crate) request_received: Cell<bool>,
    /// A Content-Length of the request's header block was malformed or too large.
    pub(crate) bad_length: Cell<bool>,
    /// The block named a Content-Type or Accept-Encoding, which are read
    /// back from `$header` once it is complete.
    pub(crate) negotiates: Cell<bool>,
    /// The coroutine waiting for the peer to open the window.
    waiting: swoole_rs_phpcoroutines::Waiter,
}

thread_local! {
    static SESSIONS: RefCell<IdMap<i64, Session>> = RefCell::new(IdMap::default());
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
        default_ctx: Ctx,
        own: Option<ServerCtx>,
        max_body_size: u32,
        handle: Handler,
    ) -> Self {
        // The peer need not send every setting: start from the defaults.
        let local = defaults();
        Self {
            fd,
            streams: RefCell::new(IdMap::default()),
            inflater: RefCell::default(),
            deflater: RefCell::default(),
            local,
            remote: local.map(Cell::new),
            remote_window_size: Cell::new(local[INIT_WINDOW_SIZE]),
            local_window_size: Cell::new(local[INIT_WINDOW_SIZE]),
            max_body_size,
            last_stream_id: Cell::new(0),
            header_stream_id: Cell::new(0),
            header_end_stream: Cell::new(false),
            shutting_down: Cell::new(false),
            default_ctx,
            own,
            handle,
        }
    }

    /// The session of connection `fd`.
    pub(crate) fn find(fd: i64) -> Option<Session> {
        SESSIONS.with_borrow(|sessions| sessions.get(&fd).cloned())
    }

    /// The session a stream's context belongs to.
    pub(crate) fn of(ctx: Ctx) -> Option<Session> {
        Self::find(ctx.fd())
    }

    /// A coroutine server's session.
    pub(crate) fn is_coro(&self) -> bool {
        self.own.is_none()
    }

    pub(crate) fn local(&self, field: usize) -> u32 {
        self.local.get(field).copied().unwrap_or(0)
    }

    /// `get_stream`, or `create_stream` when `create`.
    pub(crate) fn stream(&self, id: u32, create: bool) -> Option<Stream> {
        if !create {
            return self.streams.borrow().get(&id).cloned();
        }
        let hold = self.default_ctx.stream_context(self.fd, id);
        let stream = Rc::new(StreamState {
            ctx: hold.ctx(),
            hold,
            id,
            remote_window_size: Cell::new(self.remote[INIT_WINDOW_SIZE].get()),
            local_window_size: Cell::new(self.local[INIT_WINDOW_SIZE]),
            headers_received: Cell::new(false),
            request_received: Cell::new(false),
            bad_length: Cell::new(false),
            negotiates: Cell::new(false),
            waiting: swoole_rs_phpcoroutines::Waiter::default(),
        });
        let replaced = self.streams.borrow_mut().insert(id, stream.clone());
        drop(replaced);
        Some(stream)
    }

    /// `remove_stream`: the stream is released once nothing holds it, and a
    /// response waiting on its window resumes to find it gone.
    pub(crate) fn remove(&self, id: u32) {
        let removed = self.streams.borrow_mut().remove(&id);
        if let Some(stream) = removed {
            stream.wake();
        }
    }

    pub(crate) fn idle(&self) -> bool {
        self.streams.borrow().is_empty()
    }

    /// `handle(session, stream)`: onRequest; a bailout waits for `finish`.
    pub(crate) fn handle(&self, stream: &StreamState) {
        stream.hold.dispatch();
        bailout::defer((self.handle)(stream.ctx, self.default_ctx));
    }
}

impl StreamState {
    /// Suspend the current coroutine until `wake`.
    pub(crate) fn wait(&self) {
        self.waiting.wait();
    }

    pub(crate) fn wake(&self) {
        self.waiting.wake();
    }
}

/// A coroutine server's session for connection `fd`
/// (`swoole_http2_server_session_new`), unless it has one. `handle` gets each
/// finished request; `default_ctx` outlives the session.
pub(crate) fn session_new(fd: i64, default_ctx: Ctx, max_body_size: u32, handle: Handler) {
    insert(fd, default_ctx, None, max_body_size, handle);
}

/// An async server's session for connection `fd`, unless it has one: the
/// settings context goes with the session.
pub(crate) fn session_own(fd: i64, own: ServerCtx, max_body_size: u32, handle: Handler) {
    insert(fd, own.ctx(), Some(own), max_body_size, handle);
}

fn insert(fd: i64, default_ctx: Ctx, own: Option<ServerCtx>, max_body_size: u32, handle: Handler) {
    SESSIONS.with_borrow_mut(|sessions| {
        sessions.entry(fd).or_insert_with(|| {
            Rc::new(SessionState::new(
                fd,
                default_ctx,
                own,
                max_body_size,
                handle,
            ))
        });
    });
}

/// `swoole_http2_server_session_free`: forget connection `fd`'s session,
/// removing its streams; each is released once nothing holds it.
pub(crate) fn session_free(fd: i64) {
    let Some(session) = SESSIONS.with_borrow(|sessions| sessions.get(&fd).cloned()) else {
        return;
    };
    loop {
        let next = session.streams.borrow().keys().next().copied();
        let Some(id) = next else { break };
        session.remove(id);
    }
    let removed = SESSIONS.with_borrow_mut(|sessions| sessions.remove(&fd));
    drop(removed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_starts_from_the_default_settings() {
        // header_table_size, enable_push, max_concurrent_streams,
        // init_window_size, max_frame_size, max_header_list_size.
        assert_eq!(defaults(), [4096, 0, u32::MAX, 65535, 16384, u32::MAX]);
        assert_eq!(defaults()[INIT_WINDOW_SIZE], 65535);
    }
}
