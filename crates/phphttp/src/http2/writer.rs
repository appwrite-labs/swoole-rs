//! Serialize writes on a coroutine server connection. Its reader queues control
//! frames so it can keep receiving uploads and flow-control updates.
use super::Ctx;
use std::cell::Cell;
use swoole_rs_corebase::{reactor::TimeoutKind, timer::Deadline};
use swoole_rs_hooks::TimeoutScope;
use swoole_rs_phpcoroutines::{channel::Channel, get_cid};
use swoole_rs_zend::bailout;

pub(super) struct Writer {
    reader: Cell<i64>,
    owner: Cell<Option<(i64, Deadline)>>,
    lock: Channel<()>,
    pending: Cell<Vec<u8>>,
    partial: Cell<bool>,
    closed: Cell<bool>,
}

impl Writer {
    pub(super) fn new() -> Self {
        let lock = Channel::new();
        _ = lock.try_open(1);
        Self {
            reader: Cell::new(get_cid()),
            owner: Cell::default(),
            lock,
            pending: Cell::default(),
            partial: Cell::default(),
            closed: Cell::default(),
        }
    }

    pub(super) fn run(&self, ctx: Ctx, action: impl FnOnce() -> bool) -> bool {
        if self.closed.get() {
            return false;
        }
        let cid = get_cid();
        if self.reader.get() == cid || self.owner.get().is_some_and(|(owner, _)| owner == cid) {
            return action();
        }
        let Some(lock) = self.lock.queue() else {
            return false;
        };
        let deadline = ctx.send_deadline();
        if let Err(((), error)) = lock.clone().push((), deadline) {
            if error == swoole_rs_corebase::coroutine::chan::ERROR_TIMEOUT {
                self.abort(ctx);
                swoole_rs_corebase::log::set_last_error(super::CO_TIMEDOUT);
            }
            return false;
        }
        self.owner.set(Some((cid, deadline)));
        let ok = self.flush(ctx) && action();
        let flushed = self.flush(ctx);
        self.owner.set(None);
        _ = lock.pop(deadline);
        ok && flushed
    }

    pub(super) fn send(&self, ctx: Ctx, parts: &[&[u8]]) -> bool {
        if get_cid() == self.reader.get() {
            let mut pending = self.pending.take();
            if self.owner.get().is_some()
                && parts
                    .iter()
                    .fold(pending.len(), |len, bytes| len.saturating_add(bytes.len()))
                    > 64 * 1024
            {
                self.abort(ctx);
                return false;
            }
            for bytes in parts {
                pending.extend_from_slice(bytes);
            }
            self.pending.set(pending);
            true
        } else if self.flush(ctx) && parts.iter().all(|bytes| self.write(ctx, bytes)) {
            true
        } else {
            self.abort(ctx);
            false
        }
    }

    fn write(&self, mut ctx: Ctx, bytes: &[u8]) -> bool {
        let Some((_, deadline)) = self.owner.get() else {
            return false;
        };
        let Some(socket) = crate::context::socket(ctx.state()) else {
            return false;
        };
        let remaining = deadline.remaining();
        if remaining.is_some_and(|time| time.is_zero()) {
            swoole_rs_corebase::log::set_last_error(super::CO_TIMEDOUT);
            return false;
        }
        // Restore the socket timeout before another stream acquires the writer.
        let _timeout = TimeoutScope::new(
            socket,
            remaining.map_or(0.0, |time| time.as_secs_f64()),
            TimeoutKind::WRITE,
        );
        socket.send_all(bytes) == Some(bytes.len())
    }

    fn flush(&self, ctx: Ctx) -> bool {
        while !self.closed.get() {
            let pending = self.pending.take();
            if pending.is_empty() {
                return true;
            }
            if !self.write(ctx, &pending) {
                self.abort(ctx);
                break;
            }
            self.partial.set(false);
        }
        false
    }

    /// Try one nonblocking write before starting a background coroutine. TLS can
    /// suspend while draining encrypted bytes, so its writer takes the whole queue.
    pub(super) fn needs_flush(&self, socket: swoole_rs_hooks::co_socket::Co) -> bool {
        if self.owner.get().is_some() || self.closed.get() {
            return false;
        }
        let mut pending = self.pending.take();
        if !pending.is_empty() && socket.tls_owner().is_null() {
            match socket.send_once(&pending) {
                Some(n) if n > 0 => {
                    pending.drain(..n);
                    self.partial.set(!pending.is_empty());
                }
                None if socket
                    .catch_error(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
                    == Some(swoole_rs_corebase::reactor::Caught::Wait) => {}
                _ => {
                    self.close();
                    socket.close();
                    return false;
                }
            }
        }
        let remaining = !pending.is_empty();
        self.pending.set(pending);
        remaining
    }

    pub(super) fn finish_reader(&self, ctx: Ctx) {
        if self.reader.get() == get_cid() {
            // No more frames will be read. The reader can now wait for its final reply.
            self.reader.set(0);
            self.run(ctx, || true);
        }
    }

    pub(super) fn abort_with_goaway(&self, mut ctx: Ctx, frame: &[u8]) {
        if self.closed.replace(true) {
            return;
        }
        // A partial frame must be the last bytes on the connection.
        if self.owner.get().is_none()
            && !self.partial.get()
            && let Some(socket) = crate::context::socket(ctx.state())
        {
            // Give GOAWAY a bounded chance to drain before closing the connection.
            let _timeout = TimeoutScope::new(socket, 0.1, TimeoutKind::WRITE);
            _ = socket.send_all(frame);
        }
        ctx.close();
        self.close();
    }

    fn abort(&self, ctx: Ctx) {
        if !self.closed.replace(true) {
            ctx.close();
            self.close();
        }
    }

    pub(super) fn close(&self) {
        self.closed.set(true);
        drop(self.pending.take());
        if let Some(lock) = self.lock.release() {
            bailout::defer(lock.close().map(|_| ()));
        }
    }
}
