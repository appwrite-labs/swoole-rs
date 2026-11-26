//! `ProcessPool`'s IPC: the state its workers receive through (the stream
//! listener's address and accepted connection, the response buffer, the packet
//! buffer) and the three worker loops over it (`SW_PROTOCOL_TASK`, `_STREAM` and
//! `_MESSAGE`), plus the master's side: picking an idle worker and sending it a
//! message. Rust owns the state, each loop's receive, framing, error policy and
//! reply, and the pool's queue's `msgrcv`/`msgsnd`. Native code lends its objects
//! through [`PoolNative`]: the sockets' blocking calls (which wait and dispatch
//! signals), the message bus, the timers and the callbacks into the user's code.
//!
//! A hook that can reach PHP (a callback, a signal handler run while a socket
//! waits, the Zend allocator under the message bus) runs inside the trap the
//! extension installs with [`set_bailout`]. When PHP bails out
//! (`exit()`, a fatal error), the loop returns [`Bailed`] at once and its native
//! caller defers the bailout with [`defer_bailout`]. The outer native boundary
//! raises it after Rust cleanup has finished.
use crate::CCast;
use crate::host::DataHead;
use crate::process::process_pool::{IpcMode, Protocol};
use crate::process::worker_record::Worker as Record;
use crate::sys::set_errno;
use std::any::Any;
use std::ffi::CStr;
use std::ffi::{CString, c_int, c_long, c_void};
use std::ptr;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::constants::PipeType;
use crate::process::process_pool::PoolNative;

/// `SW_MSGMAX`: the most a queue receive asks for under the stream protocol.
const MSGMAX: usize = crate::constants::SW_MSGMAX as usize;
/// `SW_BUFFER_SIZE_STD`: the response buffer's first capacity.
const RESPONSE_CAPACITY: usize = crate::constants::SW_BUFFER_SIZE_STD as usize;
const HEAD: usize = size_of::<DataHead>();
#[cfg(test)]
const HEAD_LEN: usize = std::mem::offset_of!(DataHead, len);
const HEAD_SERVER_FD: usize = std::mem::offset_of!(DataHead, server_fd);
#[cfg(test)]
const HEAD_EXT_FLAGS: usize = std::mem::offset_of!(DataHead, ext_flags);
/// `SW_TASK_TMPFILE`: the task's data is a `PacketTask` naming a temporary file.
const TASK_TMPFILE: u16 = crate::constants::TaskFlag::SW_TASK_TMPFILE.bits();
/// `sizeof(PacketTask)`: its `size_t` length and `SW_TASK_TMP_PATH_SIZE` path.
const PACKET_TASK: usize = size_of::<super::task_file::PacketTask>();
/// `sizeof(long)`, `QueueNode::mtype`.
const MTYPE: usize = size_of::<c_long>();

/// `SW_PIPE_MASTER` and `SW_PIPE_NONBLOCK`.
pub const PIPE_MASTER: c_int = PipeType::SW_PIPE_MASTER as c_int;
pub const PIPE_NONBLOCK: c_int = PipeType::SW_PIPE_NONBLOCK as c_int;

/// A warning native code formats (`PoolNative::warn`); `errno` is the failed
/// call's where the message is a system warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warning {
    Msgrcv,
    Accept,
    Read,
    BadPacket { received: i64, length: i64 },
    TaskFailed(i64),
    Pipe,
    Send { size: i64, worker: i64 },
}

/// The `EventData` after `out`'s mtype.
fn payload(out: &mut [u8]) -> &mut [u8] {
    out.get_mut(MTYPE..).unwrap_or_default()
}

/// `bytes` written into `out` at `at`, as far as `out` reaches.
fn put(out: &mut [u8], at: usize, bytes: &[u8]) {
    out.iter_mut()
        .skip(at)
        .zip(bytes)
        .for_each(|(to, from)| *to = *from);
}

/// Whether the `n` bytes received into `out` are the whole `EventData` its head counts,
/// and a temporary file's task names its file.
fn whole_task(out: &[u8], n: isize) -> Result<(&[u8], i64), i64> {
    let event = out.get(MTYPE..).ok_or(HEAD.c_cast::<i64>())?;
    let header = event
        .first_chunk()
        .map(DataHead::from_bytes)
        .ok_or(HEAD.c_cast::<i64>())?;
    // Upstream reports the length from the worker's reusable buffer on a short read.
    let size = HEAD.c_cast::<i64>() + i64::from(header.len);
    let bytes = usize::try_from(n)
        .ok()
        .and_then(|n| event.get(..n))
        .ok_or(size)?;
    if n as i64 != size
        || (header.ext_flags & TASK_TMPFILE != 0 && header.len as usize != PACKET_TASK)
    {
        return Err(size);
    }
    Ok((bytes, header.fd))
}

/// A stream socket's native owner, which this crate keeps and hands back to the
/// hooks without knowing its type: the listener `listen` makes, or a connection
/// `accept` makes.
pub type Connection = Rc<dyn Any>;

/// One hook's arguments and results. Only this crate makes and fills one, over
/// objects live for the hook: its pool and worker (null when none), its socket
/// (null or a live native socket: a worker's pipe end), the stream connection it
/// serves (or the listener `close` frees), and the bytes it reads or fills.
pub struct PoolCall<'a> {
    pub(crate) pool: crate::process::process_pool::PoolRef<'a>,
    pub(crate) worker: Option<Record<'a>>,
    pub(crate) socket: *mut c_void,
    pub(crate) connection: Option<Connection>,
    pub(crate) buffer: Buffer<'a>,
    pub(crate) flags: c_int,
    /// The call's return value.
    pub(crate) result: isize,
    /// `errno` right after the call.
    pub(crate) error: c_int,
}

/// The bytes a hook reads ([`PoolCall::bytes`]) or fills
/// ([`PoolCall::connection_buffer`]), lent by the loop for the call.
pub(crate) enum Buffer<'a> {
    Lent(&'a [u8]),
    Fill(&'a mut [u8]),
}

impl<'a> PoolCall<'a> {
    pub(crate) fn new(
        pool: crate::process::process_pool::PoolRef<'a>,
        worker: Option<Record<'a>>,
    ) -> Self {
        Self {
            pool,
            worker,
            socket: ptr::null_mut(),
            connection: None,
            buffer: Buffer::Lent(&[]),
            flags: 0,
            result: 0,
            error: 0,
        }
    }

    /// The live pool.
    #[must_use]
    pub fn pool(&self) -> crate::process::process_pool::PoolRef<'a> {
        self.pool
    }

    /// The live worker, if the call has one.
    #[must_use]
    pub fn worker(&self) -> Option<Record<'a>> {
        self.worker
    }

    /// The worker's pipe end the hook works on, lent for this borrow of the call;
    /// `None` without one.
    #[must_use]
    pub fn lent_socket(&self) -> Option<crate::reactor::Heap<'_>> {
        crate::reactor::Heap::lend(self.socket)
    }

    /// The call's bytes as they are now; none without a buffer.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        match &self.buffer {
            Buffer::Lent(bytes) => bytes,
            Buffer::Fill(bytes) => bytes,
        }
    }

    /// The stream connection the hook serves, if any.
    #[must_use]
    pub fn connection(&self) -> Option<&dyn Any> {
        self.connection.as_deref()
    }

    /// [`PoolCall::connection`] and the bytes a read fills (none when the call only
    /// lends bytes to read), for this borrow of the call.
    pub fn connection_buffer(&mut self) -> (Option<&dyn Any>, &mut [u8]) {
        let buffer = match &mut self.buffer {
            Buffer::Fill(bytes) => &mut **bytes,
            Buffer::Lent(_) => &mut [],
        };
        (self.connection.as_deref(), buffer)
    }

    #[must_use]
    pub fn flags(&self) -> c_int {
        self.flags
    }

    /// The hook's return value.
    pub fn set_result(&mut self, result: isize) {
        self.result = result;
    }

    /// `errno` right after the hook's call.
    pub fn set_error(&mut self, error: c_int) {
        self.error = error;
    }

    /// The connection `accept` made, which the pool keeps until `close` takes it.
    pub fn set_connection(&mut self, connection: Connection) {
        self.connection = Some(connection);
    }

    /// The connection `close` frees.
    pub fn take_connection(&mut self) -> Option<Connection> {
        self.connection.take()
    }
}

/// The pool's IPC state. Native creates one per pool and frees it in `destroy`.
#[derive(Default)]
pub struct PoolIpc {
    /// `SW_IPC_SOCKET`: the listener (a native socket), its address, the
    /// connection being served and the reply the callback wrote.
    stream: Option<Stream>,
    /// The event loop's receive buffer (`ProcessPool::recv_packet`'s), allocated when
    /// a worker's loop starts, and taken out while a read fills it.
    packet: Vec<u8>,
}

struct Stream {
    listener: Option<Connection>,
    file: Option<CString>,
    port: c_int,
    connection: Option<Connection>,
    response: Option<Vec<u8>>,
}

/// The extension's bailout trap and deferred state, installed by [`set_bailout`].
pub trait Bailouts: Sync {
    /// Runs `body` inside `zend_try`: whether it bailed out.
    fn contain(&self, body: &mut dyn FnMut()) -> bool;
    /// Remember a bailout for the outer native boundary.
    fn defer(&self);
    /// Whether a callback has requested an escape.
    fn pending(&self) -> bool;
}

/// The bailout hooks [`set_bailout`] installed.
static BAILOUT: OnceLock<&'static dyn Bailouts> = OnceLock::new();

/// The hook bailed out.
pub struct Bailed;

/// `catch_system_error`: whether `error` ends a loop.
fn fatal(error: c_int) -> bool {
    !matches!(error, 0 | libc::EAGAIN | libc::EINTR | libc::ETIMEDOUT)
}

/// A [`PoolNative`] hook over its call.
type Hook = fn(&dyn PoolNative, &mut PoolCall);

/// Run `hook` over `call`, inside the trap when one is installed.
pub(crate) fn run(native: &dyn PoolNative, hook: Hook, call: &mut PoolCall) -> Result<(), Bailed> {
    trapped(|| hook(native, call))
}

/// Runs `body` inside the bailout trap, when one is installed.
pub(crate) fn trapped<R, F: FnOnce() -> R>(body: F) -> Result<R, Bailed> {
    let Some(bailouts) = BAILOUT.get() else {
        return Ok(body());
    };
    let (mut body, mut result) = (Some(body), None);
    if bailouts.contain(&mut || result = body.take().map(|body| body())) {
        return Err(Bailed);
    }
    result.ok_or(Bailed)
}

/// One worker's loop over its pool.
struct Worker<'a> {
    native: &'a dyn PoolNative,
    pool: crate::process::process_pool::PoolRef<'a>,
    worker: Record<'a>,
}

impl<'a> Worker<'a> {
    /// `f` over the pool's IPC state, borrowed per use: the worker's callbacks reach
    /// it too, as `swr_v1_process_pool_ipc_respond` does. `None` without one.
    fn with_ipc<R>(&self, f: impl FnOnce(&mut PoolIpc) -> R) -> Option<R> {
        self.pool.record().with_ipc(f)
    }

    fn call(&self) -> PoolCall<'a> {
        PoolCall::new(self.pool, Some(self.worker))
    }

    fn running(&self) -> bool {
        self.native.running(&self.call())
    }

    fn warn(&self, warning: Warning, error: c_int) {
        set_errno(error);
        self.native.warn(&self.call(), warning);
    }

    fn end(&self) -> Result<(), Bailed> {
        run(self.native, |native, _| native.end(), &mut self.call())
    }

    fn io<'b>(
        &self,
        hook: Hook,
        connection: Option<&Connection>,
        buffer: Buffer<'b>,
        flags: c_int,
    ) -> Result<PoolCall<'b>, Bailed>
    where
        'a: 'b,
    {
        let mut call: PoolCall<'b> = self.call();
        call.connection = connection.cloned();
        call.buffer = buffer;
        call.flags = flags;
        run(self.native, hook, &mut call)?;
        Ok(call)
    }

    fn close(&self, connection: Connection) {
        let mut call = self.call();
        call.connection = Some(connection);
        self.native.close(&mut call);
    }

    /// Accept a connection: the connection, or the accept's `errno`.
    fn accept(&self) -> Result<Result<Connection, c_int>, Bailed> {
        let mut call = self.call();
        run(self.native, |native, call| native.accept(call), &mut call)?;
        Ok(call.connection.take().ok_or(call.error))
    }

    /// `socket->recv_sync(data, length, MSG_WAITALL)`.
    fn recv_all(&self, socket: &Connection, data: &mut [u8]) -> Result<isize, Bailed> {
        Ok(self
            .io(
                |native, call| native.recv(call),
                Some(socket),
                Buffer::Fill(data),
                libc::MSG_WAITALL,
            )?
            .result)
    }

    fn send(&self, socket: &Connection, data: &[u8]) -> Result<isize, Bailed> {
        Ok(self
            .io(
                |native, call| native.send(call),
                Some(socket),
                Buffer::Lent(data),
                0,
            )?
            .result)
    }

    /// `Stream::recv_sync`: a length-prefixed frame of at most `data.len()` bytes.
    fn recv_frame(&self, socket: &Connection, data: &mut [u8]) -> Result<isize, Bailed> {
        let mut head = [0u8; 4];
        if self.recv_all(socket, &mut head)? != 4 {
            return Ok(-1);
        }
        let size = i32::from_be_bytes(head);
        let Some(data) = usize::try_from(size)
            .ok()
            .filter(|&size| size > 0)
            .and_then(|size| data.get_mut(..size))
        else {
            return Ok(-1);
        };
        let size = data.len().cast_signed();
        let n = self.recv_all(socket, data)?;
        Ok(if n == size { n } else { -1 })
    }

    /// Whether a stream connection is being served.
    fn connected(&self) -> bool {
        self.with_ipc(|ipc| pool_ipc_connection(ipc).is_some()) == Some(true)
    }

    fn set_connection(&self, socket: Connection) {
        self.with_ipc(|ipc| {
            if let Some(stream) = ipc.stream.as_mut() {
                stream.connection = Some(socket);
            }
        });
    }

    /// The connection being served, which the stream forgets; `None` without one.
    fn take_connection(&self) -> Option<Connection> {
        self.with_ipc(|ipc| ipc.stream.as_mut()?.connection.take())?
    }

    /// The reply written to the connection, taken.
    fn take_response(&self) -> Vec<u8> {
        self.with_ipc(|ipc| ipc.stream.as_mut()?.response.as_mut().map(std::mem::take))
            .flatten()
            .unwrap_or_default()
    }

    /// Give the reply's buffer back, emptied but keeping its capacity, as
    /// `String::clear` does.
    fn restore_response(&self, mut response: Vec<u8>) {
        response.clear();
        self.with_ipc(|ipc| {
            if let Some(stream) = ipc.stream.as_mut() {
                stream.response = Some(response);
            }
        });
    }

    /// The task hook over the `EventData` after `out`'s mtype.
    fn task(&self, (bytes, fd): (&[u8], i64)) {
        let mut call = self.call();
        call.buffer = Buffer::Lent(bytes);
        self.native.task(&mut call);
        if call.result < 0 {
            self.warn(Warning::TaskFailed(fd), 0);
        }
    }

    /// `SW_PROTOCOL_TASK`: one `EventData` per message.
    fn run_task(
        &self,
        mode: Mode,
        event_size: usize,
        by_sysvmsg: bool,
        worker_id: u32,
    ) -> Result<(), Bailed> {
        // `struct { long mtype; EventData buf; }`, zeroed.
        let mut out = vec![0u8; MTYPE + event_size];
        let mtype: u64 = if by_sysvmsg {
            0
        } else {
            u64::from(worker_id) + 1
        };
        put(&mut out, 0, &mtype.to_ne_bytes());
        put(
            &mut out,
            MTYPE + HEAD_SERVER_FD,
            &worker_id.c_cast::<u16>().to_ne_bytes(),
        );

        while self.running() {
            let n = match mode {
                Mode::Queue => {
                    // The received mtype stays in `out` for the next receive, as upstream.
                    let call = self.call();
                    let (n, error) = trapped(|| call.pool().queue_pop(&mut out, event_size))?;
                    if n < 0 && fatal(error) {
                        self.warn(Warning::Msgrcv, error);
                        break;
                    }
                    n
                }
                Mode::Socket => {
                    let socket = match self.accept()? {
                        Ok(socket) => socket,
                        Err(error) if fatal(error) => {
                            self.warn(Warning::Accept, error);
                            break;
                        }
                        Err(_) => {
                            self.end()?;
                            continue;
                        }
                    };
                    let n = self.recv_frame(&socket, payload(&mut out))?;
                    if n <= 0 {
                        self.close(socket);
                        self.end()?;
                        continue;
                    }
                    self.set_connection(socket);
                    n
                }
                Mode::Pipe => {
                    // A whole task with no connection to answer runs under the read's
                    // own trap, so the iteration sets one trap, not two.
                    let (ran, result, error) = trapped(|| {
                        let mut read = self.call();
                        read.buffer = Buffer::Fill(payload(&mut out));
                        self.native.pipe_read(&mut read);
                        let (result, error) = (read.result, read.error);
                        let task = whole_task(&out, result).ok().filter(|_| !self.connected());
                        if let Some(task) = task {
                            self.task(task);
                            self.native.end();
                        }
                        (task.is_some(), result, error)
                    })?;
                    if ran {
                        continue;
                    }
                    if result < 0 && fatal(error) {
                        self.warn(Warning::Read, error);
                        break;
                    }
                    result
                }
            };
            if n >= 0 {
                let alone = !self.connected();
                match whole_task(&out, n) {
                    Ok(task) => {
                        // Without a connection to answer, nothing comes between the task
                        // and the iteration's end, so one trap covers both.
                        trapped(|| {
                            self.task(task);
                            if alone {
                                self.native.end();
                            }
                        })?;
                        if alone {
                            continue;
                        }
                    }
                    Err(length) => self.warn(
                        Warning::BadPacket {
                            received: n as i64,
                            length,
                        },
                        0,
                    ),
                }
                if let Some(socket) = self.take_connection() {
                    self.send(&socket, &0i32.to_ne_bytes())?;
                    self.close(socket);
                }
            }
            self.end()?;
        }
        Ok(())
    }

    /// `SW_PROTOCOL_STREAM`: raw messages, length-prefixed over the stream socket.
    fn run_stream(&self, mode: Mode, max_packet_size: u32) -> Result<(), Bailed> {
        // A zeroed buffer: the `QueueNode` mtype is 0.
        let Some(mut packet) = self.with_ipc(|ipc| ipc.prepare(max_packet_size)) else {
            return Ok(());
        };
        let most = (max_packet_size as usize).min(packet.len());

        while self.running() {
            // Where the message starts in `packet`, and its length.
            let (at, n) = match mode {
                Mode::Queue => {
                    let call = self.call();
                    let (n, error) = trapped(|| call.pool().queue_pop(&mut packet, MSGMAX))?;
                    if n < 0 && fatal(error) {
                        self.warn(Warning::Msgrcv, error);
                        break;
                    }
                    put(&mut packet, 0, &[0; MTYPE]);
                    (MTYPE, n)
                }
                Mode::Socket => {
                    let socket = match self.accept()? {
                        Ok(socket) => socket,
                        Err(error) if fatal(error) => {
                            self.warn(Warning::Accept, error);
                            break;
                        }
                        Err(_) => {
                            self.end()?;
                            continue;
                        }
                    };
                    let mut head = [0u8; 4];
                    let mut received = self.recv_all(&socket, &mut head)? == 4;
                    let n = u32::from_be_bytes(head);
                    // Errors on one connection close it and carry on with the next.
                    received = received && n != 0 && n <= max_packet_size;
                    let data = packet.get_mut(..n as usize).filter(|_| received);
                    let Some(data) = data else {
                        self.close(socket);
                        self.end()?;
                        continue;
                    };
                    if self.recv_all(&socket, data)? != n.c_cast::<isize>() {
                        self.close(socket);
                        self.end()?;
                        continue;
                    }
                    self.set_connection(socket);
                    (0, n.c_cast::<isize>())
                }
                Mode::Pipe => {
                    let call = self.io(
                        |native, call| native.pipe_read(call),
                        None,
                        Buffer::Fill(packet.get_mut(..most).unwrap_or_default()),
                        0,
                    )?;
                    if call.result < 0 && fatal(call.error) {
                        self.warn(Warning::Read, call.error);
                        break;
                    }
                    (0, call.result)
                }
            };
            if let Ok(length) = usize::try_from(n)
                && let Some(data) = packet.get(at..).and_then(|rest| rest.get(..length))
            {
                self.io(
                    |native, call| native.message(call),
                    None,
                    Buffer::Lent(data),
                    0,
                )?;
                if let Some(socket) = self.take_connection() {
                    let response = self.take_response();
                    if !response.is_empty() {
                        let head = response.len().c_cast::<u32>().to_be_bytes();
                        self.send(&socket, &head)?;
                        self.send(&socket, &response)?;
                    }
                    self.restore_response(response);
                    self.close(socket);
                }
            }
            self.end()?;
        }
        Ok(())
    }

    /// `SW_PROTOCOL_MESSAGE`: the message bus over the worker's pipe.
    fn run_message(&self) -> Result<(), Bailed> {
        while self.running() {
            match self.receive_message()? {
                Received::Continue | Received::Ready => self.end()?,
                Received::Error(error) => {
                    self.warn(Warning::Pipe, error);
                    self.native.shutdown(&self.call());
                }
            }
        }
        Ok(())
    }

    fn receive_message(&self) -> Result<Received, Bailed> {
        let interrupted = |error: c_int| {
            if error == libc::EINTR {
                Received::Continue
            } else {
                Received::Error(error)
            }
        };
        loop {
            let mut call = self.call();
            run(
                self.native,
                |native, call| native.pipe_wait(call),
                &mut call,
            )?;
            if call.result < 0 {
                return Ok(if fatal(call.error) {
                    Received::Error(call.error)
                } else {
                    Received::Continue
                });
            }
            let mut call = self.call();
            run(self.native, |native, call| native.bus_read(call), &mut call)?;
            match call.result {
                n if n < 0 => return Ok(interrupted(call.error)),
                0 => {}
                _ => {
                    run(
                        self.native,
                        |native, call| native.bus_deliver(call),
                        &mut self.call(),
                    )?;
                    return Ok(Received::Ready);
                }
            }
        }
    }
}

enum Received {
    Continue,
    Ready,
    Error(c_int),
}

#[derive(Clone, Copy)]
enum Mode {
    Pipe,
    Queue,
    Socket,
}

impl PoolIpc {
    /// A zeroed packet buffer a worker receives into, and, for a stream listener,
    /// the response buffer.
    fn prepare(&mut self, max_packet_size: u32) -> Vec<u8> {
        // A queue receive reads up to SW_MSGMAX bytes after the mtype.
        let size = (max_packet_size as usize).max(MTYPE + MSGMAX);
        if let Some(stream) = self.stream.as_mut() {
            stream.response = Some(Vec::with_capacity(RESPONSE_CAPACITY));
        }
        vec![0; size]
    }
}

/// Install the extension's bailout hooks: the first installed stays.
pub fn set_bailout(bailouts: &'static dyn Bailouts) {
    _ = BAILOUT.set(bailouts);
}

/// Defer the bailout a loop returned [`BAILED`] for until its native boundary.
pub fn defer_bailout() {
    if let Some(bailouts) = BAILOUT.get() {
        bailouts.defer();
    }
}

/// Whether a PHP callback requested that its enclosing operation stop.
#[must_use]
pub fn bailout_pending() -> bool {
    BAILOUT.get().is_some_and(|bailouts| bailouts.pending())
}

/// A pool's IPC state. `listener`: the pool uses a stream socket (`SW_IPC_SOCKET`).
#[must_use]
pub fn pool_ipc_new(listener: bool) -> *mut PoolIpc {
    let stream = listener.then_some(Stream {
        listener: None,
        file: None,
        port: 0,
        connection: None,
        response: None,
    });
    Box::into_raw(Box::new(PoolIpc {
        stream,
        packet: Vec::new(),
    }))
}

/// Free the state, which the caller takes back from the handle [`pool_ipc_new`]
/// leaked; `close` frees the listener. A listener's address is a socket file,
/// unlinked as upstream does (a TCP host is unlinked too, harmlessly).
pub fn pool_ipc_free(ipc: PoolIpc, close: impl FnOnce(Connection)) {
    if let Some(stream) = ipc.stream
        && let Some(listener) = stream.listener
    {
        if let Some(file) = &stream.file {
            _ = crate::sys::unlink(file);
        }
        close(listener);
    }
}

/// Record a stream listener's address before it is created: false when the pool
/// has no stream socket.
pub(crate) fn swr_v1_process_pool_ipc_address(ipc: &mut PoolIpc, host: &CStr, port: c_int) -> bool {
    let Some(stream) = ipc.stream.as_mut() else {
        return false;
    };
    stream.file = Some(host.to_owned());
    stream.port = port;
    true
}

/// `ProcessPool::dispatch_sync(data, len)` (`blocking`): connect to the stream listener, send
/// `data` length-prefixed (big-endian) and close; false on any failure. A unix
/// listener records port 0, which a TCP one can't be reached on.
///
/// `ProcessPool::dispatch(data)`: the same over a [`crate::process::stream::Stream`], which, as upstream's,
/// connects a Unix socket whatever the listener's port, and fails only when that connect does.
///
pub(crate) fn swr_v1_process_pool_ipc_send(
    ipc: Option<&PoolIpc>,
    data: &[u8],
    blocking: bool,
) -> bool {
    use std::io::Write;
    fn send(mut socket: impl Write, data: &[u8]) -> std::io::Result<()> {
        let len = u32::try_from(data.len()).map_err(|_| std::io::ErrorKind::InvalidInput)?;
        socket.write_all(&len.to_be_bytes())?;
        socket.write_all(data)
    }
    let Some(stream) = ipc.and_then(|ipc| ipc.stream.as_ref()) else {
        return false;
    };
    let Some(file) = &stream.file else {
        return false;
    };
    if !blocking {
        let Some(mut stream) = crate::process::stream::Stream::create(
            &crate::process::stream::Peer::Unix(file.as_bytes()),
        ) else {
            return false;
        };
        stream.send(data);
        stream.flush();
        return true;
    }
    if stream.port == 0 {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::OsStr::from_bytes(file.as_bytes());
        return std::os::unix::net::UnixStream::connect(path)
            .and_then(|socket| send(socket, data))
            .is_ok();
    }
    let (Ok(host), Ok(port)) = (file.to_str(), u16::try_from(stream.port)) else {
        return false;
    };
    std::net::TcpStream::connect((host, port))
        .and_then(|socket| send(socket, data))
        .is_ok()
}

/// `ProcessPool::dispatch_sync(data, len)`: [`swr_v1_process_pool_ipc_send`]'s blocking
/// send of raw bytes. Only core-tests' `os/process_pool.cpp` sends raw bytes; its Rust port
/// (`tests/modules/hooks/system/src/process_pool.rs`) calls this.
#[must_use]
pub fn swr_v1_process_pool_ipc_send_sync(ipc: Option<&PoolIpc>, data: &[u8]) -> bool {
    swr_v1_process_pool_ipc_send(ipc, data, true)
}

/// Keep the listener the native side created.
pub(crate) fn swr_v1_process_pool_ipc_set_listener(ipc: &mut PoolIpc, listener: Connection) {
    if let Some(stream) = ipc.stream.as_mut() {
        stream.listener = Some(listener);
    }
}

/// The stream listener, if any.
#[must_use]
pub fn swr_v1_process_pool_ipc_listener(ipc: &PoolIpc) -> Option<&Connection> {
    ipc.stream.as_ref()?.listener.as_ref()
}

/// The listener's address and port, none without one.
#[cfg(test)]
pub(crate) fn pool_ipc_address(ipc: &PoolIpc) -> Option<(&CStr, c_int)> {
    let stream = ipc.stream.as_ref()?;
    Some((stream.file.as_deref()?, stream.port))
}

/// The connection being served (`stream_info_->last_connection`), if any.
#[must_use]
pub fn pool_ipc_connection(ipc: &PoolIpc) -> Option<Connection> {
    ipc.stream.as_ref()?.connection.clone()
}

/// `ProcessPool::response`: append to the reply; false (`SW_ERROR_INVALID_PARAMS`
/// for the caller) when there is nothing to send or no connection to reply to.
pub(crate) fn swr_v1_process_pool_ipc_respond(ipc: &mut PoolIpc, data: &[u8]) -> bool {
    let Some(stream) = ipc.stream.as_mut() else {
        return false;
    };
    if data.is_empty() || stream.connection.is_none() {
        return false;
    }
    let Some(response) = stream.response.as_mut() else {
        return false;
    };
    response.extend_from_slice(data);
    true
}

/// Allocate a worker's receive buffers for its event loop.
pub fn pool_ipc_prepare(ipc: &mut PoolIpc, max_packet_size: u32) {
    ipc.packet = ipc.prepare(max_packet_size);
}

/// The packet buffer `_prepare` allocated, taken out until
/// [`pool_ipc_restore_packet`]; empty before, or while taken.
pub(crate) fn pool_ipc_take_packet(ipc: &mut PoolIpc) -> Vec<u8> {
    std::mem::take(&mut ipc.packet)
}

/// Give the packet buffer back.
pub(crate) fn pool_ipc_restore_packet(ipc: &mut PoolIpc, packet: Vec<u8>) {
    ipc.packet = packet;
}

fn mode(ipc_mode: IpcMode) -> Mode {
    match ipc_mode {
        IpcMode::MsgQueue => Mode::Queue,
        IpcMode::Socket => Mode::Socket,
        IpcMode::UnixSock | IpcMode::None => Mode::Pipe,
    }
}

/// What a worker loop needs to know about its pool, which holds the loop's IPC
/// state, and its worker.
pub struct PoolLoop<'a> {
    pub pool: crate::process::process_pool::PoolRef<'a>,
    pub worker: Record<'a>,
    pub worker_id: u32,
    /// `use_msgqueue`/`use_socket` as an IPC mode.
    pub ipc_mode: IpcMode,
    pub schedule_by_sysvmsg: bool,
    pub max_packet_size: u32,
    /// `sizeof(EventData)`.
    pub event_size: usize,
}

/// `ProcessPool::run_with_task_protocol`/`_stream_protocol`/`_message_protocol`,
/// between `at_worker_enter` and `at_worker_exit`. A pool `create` never made has
/// no IPC state and is done at once.
///
/// # Errors
///
/// [`Bailed`] when PHP bailed out inside a hook; the caller raises it at once.
pub fn pool_run(
    native: &dyn PoolNative,
    pool_loop: &PoolLoop<'_>,
    protocol: Protocol,
) -> Result<(), Bailed> {
    if pool_loop.pool.ipc().is_null() {
        return Ok(());
    }
    let worker = Worker {
        native,
        pool: pool_loop.pool,
        worker: pool_loop.worker,
    };
    let mode = mode(pool_loop.ipc_mode);
    match protocol {
        Protocol::Task => worker.run_task(
            mode,
            pool_loop.event_size,
            pool_loop.schedule_by_sysvmsg,
            pool_loop.worker_id,
        ),
        Protocol::Stream => worker.run_stream(mode, pool_loop.max_packet_size),
        Protocol::Message => worker.run_message(),
    }
}

/// `ProcessPool::schedule`'s round robin over `count` (> 0) workers: the first
/// idle one from the next round, or the last one tried and `true` (the pool's
/// scheduler warning) when all are busy. `round` is the pool's shared counter.
pub fn pool_schedule(count: u32, round: &AtomicU32, idle: impl Fn(u32) -> bool) -> (u32, bool) {
    let mut target = 0;
    for _ in 0..=count {
        target = round.fetch_add(1, Ordering::SeqCst) % count;
        if idle(target) {
            return (target, false);
        }
    }
    (target, true)
}

/// Whether `worker` is `SW_WORKER_IDLE`.
pub(crate) fn idle(worker: Record<'_>) -> bool {
    worker.is_idle()
}

/// `Worker::send_pipe_message` of `data`: through the pool's queue (as `mtype`
/// worker id + 1) when it has one, else `pipe`, the master's or the worker's end of
/// worker `worker_id`'s pipe ([`Record::pipe_master`], [`Record::pipe_worker`]).
pub(crate) fn swr_v1_process_worker_send(
    native: &dyn PoolNative,
    pool: crate::process::process_pool::PoolRef<'_>,
    pipe: *mut c_void,
    queue: bool,
    worker_id: u32,
    data: &[u8],
    flags: c_int,
) -> Option<usize> {
    let length = data.len();
    let mut call = PoolCall::new(pool, None);
    if queue {
        // `mtype`, then `data`.
        let mtype = c_long::from(worker_id) + 1;
        let message = [&mtype.to_ne_bytes()[..], data].concat();
        // The queue's push, which cannot reach PHP.
        return pool.queue_push(&message).then_some(length);
    }
    if pipe.is_null() {
        // A worker without a pipe, which upstream doesn't send through.
        return None;
    }
    // A worker's pipe end, a live socket (the record's invariant).
    call.socket = pipe;
    call.buffer = Buffer::Lent(data);
    call.flags = flags & PIPE_NONBLOCK;
    // The native pipe write, which cannot reach PHP.
    native.pipe_write(&mut call);
    set_errno(call.error);
    usize::try_from(call.result).ok()
}

/// `ProcessPool::dispatch`/`dispatch_sync` to a worker's pipe or queue: offset
/// the chosen worker `*target` by `start_id`, and send `event` (`blocking`: wait
/// for the pipe). False after reporting a failed send.
#[expect(
    clippy::too_many_arguments,
    reason = "the flat list of the former C export; `Pool::dispatch` is the one caller"
)]
pub(crate) fn pool_dispatch<'a>(
    native: &dyn PoolNative,
    pool: crate::process::process_pool::PoolRef<'_>,
    workers: impl Fn(usize) -> Option<Record<'a>>,
    start_id: u16,
    queue: bool,
    index: u32,
    event: &[u8],
    blocking: bool,
) -> bool {
    let size = u32::try_from(event.len()).unwrap_or(u32::MAX);
    // The worker's id, as upstream's `*dst_worker_id += start_id` leaves it.
    let target = i64::from(index) + i64::from(start_id);
    let Some(worker) = workers(index as usize) else {
        let call = PoolCall::new(pool, None);
        native.warn(
            &call,
            Warning::Send {
                size: i64::from(size),
                worker: target,
            },
        );
        return false;
    };
    let (id, pipe) = (worker.id(), worker.pipe_master());
    let flags = PIPE_MASTER | if blocking { 0 } else { PIPE_NONBLOCK };
    if swr_v1_process_worker_send(native, pool, pipe, queue, id, event, flags).is_none() {
        let call = PoolCall::new(pool, None);
        native.warn(
            &call,
            Warning::Send {
                size: i64::from(size),
                worker: target,
            },
        );
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fatal_errors_end_the_loop() {
        assert!(!fatal(0));
        assert!(!fatal(libc::EAGAIN));
        assert!(!fatal(libc::EINTR));
        assert!(!fatal(libc::ETIMEDOUT));
        assert!(fatal(libc::EBADF));
    }

    #[test]
    fn respond_needs_a_connection_and_a_buffer() {
        let ipc = pool_ipc_new(true);
        // SAFETY: a live handle.
        unsafe {
            assert!(!swr_v1_process_pool_ipc_respond(&mut *ipc, b"x"));
            pool_ipc_prepare(&mut *ipc, 16);
            assert!(!swr_v1_process_pool_ipc_respond(&mut *ipc, b"x"));
            (*ipc).stream.as_mut().unwrap().connection = Some(Rc::new(()));
            assert!(!swr_v1_process_pool_ipc_respond(&mut *ipc, b""));
            assert!(swr_v1_process_pool_ipc_respond(&mut *ipc, b"ab"));
            assert_eq!(
                (*ipc).stream.as_ref().unwrap().response.as_deref(),
                Some(&b"ab"[..])
            );
            (*ipc).stream.as_mut().unwrap().connection = None;
            pool_ipc_free(*Box::from_raw(ipc), |_| {});
        }
    }

    #[test]
    fn a_pool_without_a_listener_has_no_address() {
        let ipc = pool_ipc_new(false);
        // SAFETY: a live handle.
        unsafe {
            assert!(!swr_v1_process_pool_ipc_address(&mut *ipc, c"/tmp/x", 1));
            assert!(pool_ipc_address(&*ipc).is_none());
            assert!(swr_v1_process_pool_ipc_listener(&*ipc).is_none());
            pool_ipc_free(*Box::from_raw(ipc), |_| {});
        }
    }

    #[test]
    fn the_packet_buffer_holds_a_queue_message() {
        let mut ipc = PoolIpc::default();
        assert!(ipc.prepare(64).len() >= MTYPE + MSGMAX);
        assert!(ipc.prepare(4 << 20).len() >= 4 << 20);
    }

    fn records(busy: &[bool]) -> Vec<crate::process::worker_record::WorkerRecord> {
        busy.iter()
            .map(|&busy| {
                let mut record = *crate::process::worker_record::WorkerRecord::boxed();
                record.set_status(crate::process::worker_record::WorkerStatus::busy(busy));
                record
            })
            .collect()
    }

    fn schedule(
        workers: &mut [crate::process::worker_record::WorkerRecord],
        round: &AtomicU32,
    ) -> (u32, bool) {
        let count = u32::try_from(workers.len()).unwrap();
        let workers: Vec<_> = workers.iter_mut().map(Record::from).collect();
        pool_schedule(count, round, |target| idle(workers[target as usize]))
    }

    #[test]
    fn schedule_picks_the_next_idle_worker() {
        let mut workers = records(&[true, false, true]);
        let round = AtomicU32::new(0);
        let (target, warning) = schedule(&mut workers, &round);
        assert_eq!((target, round.into_inner(), warning), (1, 2, false));
    }

    #[test]
    fn schedule_warns_when_every_worker_is_busy() {
        let mut workers = records(&[true, true]);
        let round = AtomicU32::new(5);
        let (target, warning) = schedule(&mut workers, &round);
        // Three tries (count + 1): rounds 5, 6, 7; the last is 7 % 2.
        assert_eq!((target, round.into_inner(), warning), (1, 8, true));
    }

    /// A scripted native side: what each hook returns, and what was seen.
    #[derive(Default)]
    struct Script {
        rounds: u32,
        ops: Vec<(crate::process::process_pool::Op, u64)>,
        accepts: Vec<usize>,
        recvs: Vec<Vec<u8>>,
        reads: Vec<(Vec<u8>, c_int)>,
        task_result: isize,
        bus: Vec<(isize, c_int)>,
        sent: Vec<Vec<u8>>,
        closed: Vec<usize>,
        messages: Vec<Vec<u8>>,
        tasks: usize,
        delivered: usize,
        ends: usize,
        shutdowns: usize,
        warnings: Vec<Warning>,
        respond: Vec<u8>,
        ipc: usize,
    }

    thread_local! {
        static SCRIPT: std::cell::RefCell<Script> = std::cell::RefCell::default();
    }

    fn with<R>(f: impl FnOnce(&mut Script) -> R) -> R {
        SCRIPT.with(|script| f(&mut script.borrow_mut()))
    }

    fn fill(call: &mut PoolCall, bytes: &[u8]) {
        put(call.connection_buffer().1, 0, bytes);
        call.result = bytes.len().c_cast::<isize>();
    }

    fn bytes_of(call: &PoolCall) -> Vec<u8> {
        call.bytes().to_vec()
    }

    /// The scripted hooks, over the thread's [`Script`].
    pub(super) struct Scripted;

    pub(super) fn take_ops() -> Vec<(crate::process::process_pool::Op, u64)> {
        with(|script| std::mem::take(&mut script.ops))
    }

    impl PoolNative for Scripted {
        fn call(&self, _: &PoolCall, op: crate::process::process_pool::Op, arg: u64) {
            with(|script| script.ops.push((op, arg)));
        }
        fn event_init(&self, _: &PoolCall) -> bool {
            unreachable!("loop-only fixture")
        }
        fn event_available(&self) -> bool {
            unreachable!("loop-only fixture")
        }
        fn event_wait(&self) -> bool {
            unreachable!("loop-only fixture")
        }
        fn waitpid(&self, _: c_int) -> bool {
            unreachable!("loop-only fixture")
        }
        fn new_pipe(&self, _: &PoolCall) -> Result<(), crate::process::process_pool::PipeError> {
            unreachable!("loop-only fixture")
        }
        fn add_pipe(&self, _: &PoolCall) -> Result<(), crate::process::process_pool::PipeError> {
            unreachable!("loop-only fixture")
        }
        fn push_stop(&self, _: &PoolCall) -> Result<(), crate::process::process_pool::PipeError> {
            unreachable!("loop-only fixture")
        }
        fn send_buffer_size(&self, _: &PoolCall) -> std::io::Result<c_int> {
            unreachable!("loop-only fixture")
        }
        fn bus_buffer(
            &self,
            _: &PoolCall,
            _: usize,
        ) -> Result<(), crate::process::process_pool::PipeError> {
            unreachable!("loop-only fixture")
        }
        fn alloc_workers(&self, _: u32) -> *mut crate::process::worker_record::WorkerRecord {
            core::ptr::null_mut()
        }
        fn new_message_bus(&self) -> Option<Box<dyn std::any::Any>> {
            None
        }
        fn callback(&self, _: &mut PoolCall, _: crate::process::process_pool::Callback) {}
        fn event_size(&self) -> usize {
            128
        }
        fn running(&self, _: &PoolCall) -> bool {
            with(|s| {
                s.rounds = s.rounds.saturating_sub(1);
                s.rounds > 0
            })
        }
        fn end(&self) {
            with(|s| s.ends += 1);
        }
        fn accept(&self, c: &mut PoolCall) {
            let next = with(|s| s.accepts.pop());
            if let Some(id) = next {
                c.set_connection(Rc::new(id));
            }
            c.error = libc::EAGAIN;
        }
        fn recv(&self, c: &mut PoolCall) {
            let next = with(|s| s.recvs.remove(0));
            fill(c, &next);
        }
        fn send(&self, c: &mut PoolCall) {
            let bytes = bytes_of(c);
            c.result = bytes.len().c_cast::<isize>();
            with(|s| s.sent.push(bytes));
        }
        fn close(&self, c: &mut PoolCall) {
            let id = c
                .take_connection()
                .and_then(|c| c.downcast_ref::<usize>().copied());
            with(|s| s.closed.extend(id));
        }
        fn pipe_read(&self, c: &mut PoolCall) {
            let (bytes, error) = with(|s| s.reads.remove(0));
            if error == 0 {
                fill(c, &bytes);
            } else {
                c.result = -1;
                c.error = error;
            }
        }
        fn pipe_wait(&self, c: &mut PoolCall) {
            c.result = 0;
        }
        fn bus_write(
            &self,
            _: crate::process::process_pool::PoolRef<'_>,
            _: Record<'_>,
            _: &mut crate::DataHead,
            _: &[u8],
        ) -> bool {
            false
        }
        fn bus_read(&self, c: &mut PoolCall) {
            let (result, error) = with(|s| s.bus.remove(0));
            c.result = result;
            c.error = error;
        }
        fn bus_deliver(&self, _: &mut PoolCall) {
            with(|s| s.delivered += 1);
        }
        fn task(&self, c: &mut PoolCall) {
            c.result = with(|s| {
                s.tasks += 1;
                s.task_result
            });
        }
        fn message(&self, c: &mut PoolCall) {
            let bytes = bytes_of(c);
            let (ipc, respond) = with(|s| {
                s.messages.push(bytes);
                (s.ipc as *mut PoolIpc, std::mem::take(&mut s.respond))
            });
            if !respond.is_empty() {
                // SAFETY: the test's handle.
                let responded = swr_v1_process_pool_ipc_respond(unsafe { &mut *ipc }, &respond);
                assert!(responded);
            }
        }
        fn shutdown(&self, _: &PoolCall) {
            with(|s| s.shutdowns += 1);
        }
        fn pipe_write(&self, _: &mut PoolCall) {}
        fn warn(&self, _: &PoolCall, warning: Warning) {
            with(|s| s.warnings.push(warning));
        }
    }

    fn run_loop(script: Script, listener: bool, ipc_mode: IpcMode, protocol: Protocol) -> Script {
        let ipc = pool_ipc_new(listener);
        with(|s| {
            *s = script;
            s.ipc = ipc as usize;
        });
        let pool = crate::process::process_pool::PoolRecord::empty();
        pool.ipc.set(ipc);
        let pool = std::cell::UnsafeCell::new(pool);
        let mut worker = crate::process::worker_record::WorkerRecord::empty();
        let pool_loop = PoolLoop {
            pool: (&pool).into(),
            worker: (&mut worker).into(),
            worker_id: 3,
            ipc_mode,
            schedule_by_sysvmsg: false,
            max_packet_size: 64,
            event_size: 128,
        };
        assert!(pool_run(&Scripted, &pool_loop, protocol).is_ok());
        // SAFETY: as above.
        pool_ipc_free(*unsafe { Box::from_raw(ipc) }, |_| {});
        with(std::mem::take)
    }

    #[test]
    fn the_stream_protocol_replies_with_a_framed_response() {
        let script = run_loop(
            Script {
                rounds: 3,
                accepts: vec![7],
                recvs: vec![3u32.to_be_bytes().to_vec(), b"abc".to_vec()],
                respond: b"ok".to_vec(),
                ..Script::default()
            },
            true,
            IpcMode::Socket,
            Protocol::Stream,
        );
        assert_eq!(script.messages, vec![b"abc".to_vec()]);
        assert_eq!(
            script.sent,
            vec![2u32.to_be_bytes().to_vec(), b"ok".to_vec()]
        );
        // The connection, then nothing to accept (EAGAIN): the loop carries on.
        assert_eq!(script.closed, vec![7]);
        assert_eq!(script.ends, 2);
        assert_eq!(script.warnings, []);
    }

    #[test]
    fn the_stream_protocol_drops_an_oversized_frame() {
        let script = run_loop(
            Script {
                rounds: 2,
                accepts: vec![9],
                recvs: vec![65u32.to_be_bytes().to_vec()],
                ..Script::default()
            },
            true,
            IpcMode::Socket,
            Protocol::Stream,
        );
        assert_eq!(script.messages, Vec::<Vec<u8>>::new());
        assert_eq!(script.closed, vec![9]);
        assert_eq!(script.ends, 1);
    }

    #[test]
    fn the_task_protocol_skips_a_bad_packet_and_reports_a_failed_task() {
        let packet = |length: u32, size: usize, ext_flags: u16| {
            let mut packet = vec![0u8; size];
            packet[..8].copy_from_slice(&42i64.to_ne_bytes());
            packet[HEAD_LEN..HEAD_LEN + 4].copy_from_slice(&length.to_ne_bytes());
            packet[HEAD_EXT_FLAGS..HEAD_EXT_FLAGS + 2].copy_from_slice(&ext_flags.to_ne_bytes());
            packet
        };
        let short = packet(4, HEAD + 2, 0);
        let tmpfile = packet(4, HEAD + 4, TASK_TMPFILE);
        let good = packet(4, HEAD + 4, 0);
        let script = run_loop(
            Script {
                rounds: 5,
                reads: vec![
                    (short, 0),
                    (tmpfile, 0),
                    (good, 0),
                    (Vec::new(), libc::EBADF),
                ],
                task_result: -1,
                ..Script::default()
            },
            false,
            IpcMode::UnixSock,
            Protocol::Task,
        );
        // Neither bad packet reaches the task.
        assert_eq!(script.tasks, 1);
        assert_eq!(
            script.warnings,
            vec![
                Warning::BadPacket {
                    received: HEAD.c_cast::<i64>() + 2,
                    length: HEAD.c_cast::<i64>() + 4
                },
                Warning::BadPacket {
                    received: HEAD.c_cast::<i64>() + 4,
                    length: HEAD.c_cast::<i64>() + 4
                },
                Warning::TaskFailed(42),
                Warning::Read,
            ]
        );
        // Each packet ends its iteration; the failed read ends the loop before one.
        assert_eq!(script.ends, 3);
    }

    #[test]
    fn the_message_protocol_shuts_the_worker_down_on_a_pipe_error() {
        let script = run_loop(
            Script {
                rounds: 4,
                bus: vec![(0, 0), (1, 0), (-1, libc::EINTR), (-1, libc::EIO)],
                ..Script::default()
            },
            false,
            IpcMode::UnixSock,
            Protocol::Message,
        );
        assert_eq!(script.delivered, 1);
        // The delivered message and the interrupted read each end an iteration.
        assert_eq!(script.ends, 2);
        assert_eq!(script.shutdowns, 1);
        assert_eq!(script.warnings, vec![Warning::Pipe]);
    }

    #[test]
    fn a_dispatch_to_a_worker_the_pool_lacks_warns_and_fails() {
        with(|s| *s = Script::default());
        let mut workers = records(&[true, true]);
        let workers: Vec<_> = workers.iter_mut().map(Record::from).collect();
        let event = [0u8; 4];
        let mut pool = crate::process::process_pool::PoolRecord::empty();
        let sent = pool_dispatch(
            &Scripted,
            (&mut pool).into(),
            |index| workers.get(index).copied(),
            0,
            false,
            5,
            &event,
            false,
        );
        assert!(!sent);
        assert_eq!(
            with(|s| s.warnings.clone()),
            vec![Warning::Send { size: 4, worker: 5 }]
        );
    }
}

#[cfg(test)]
#[path = "../../../../tests/modules/process/native_methods.rs"]
mod native_methods_tests;
