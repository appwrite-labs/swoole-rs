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
//! caller raises the bailout again with [`raise_bailout`], so the
//! `longjmp` never crosses a Rust frame.
use crate::process_pool::{IpcMode, Protocol};
use crate::worker_record::Worker as Record;
#[cfg(any(feature = "upstream-symbols", test))]
use std::ffi::CStr;
use std::ffi::{CString, c_char, c_int, c_long, c_void};
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::sys::set_errno;

use crate::process_pool::PoolNative;

/// `SW_MSGMAX`: the most a queue receive asks for under the stream protocol.
const MSGMAX: usize = 65536;
/// `SW_BUFFER_SIZE_STD`: the response buffer's first capacity.
const RESPONSE_CAPACITY: usize = 8192;
/// `sizeof(swoole::DataHead)`; `info.len` is its `u32` at offset 16, `info.fd`
/// (the task id) its `i64` at 0, `info.server_fd` its `u16` at 24 and
/// `info.ext_flags` its `u16` at 26.
const HEAD: usize = 40;
const HEAD_LEN: usize = 16;
const HEAD_SERVER_FD: usize = 24;
const HEAD_EXT_FLAGS: usize = 26;
/// `SW_TASK_TMPFILE`: the task's data is a `PacketTask` naming a temporary file.
const TASK_TMPFILE: u16 = 1;
/// `sizeof(PacketTask)`: its `size_t` length and `SW_TASK_TMP_PATH_SIZE` path.
const PACKET_TASK: u32 = 8 + 256;
/// `sizeof(long)`, `QueueNode::mtype`.
const MTYPE: usize = size_of::<c_long>();

/// `SW_PIPE_MASTER` and `SW_PIPE_NONBLOCK`.
const PIPE_MASTER: c_int = 1;
pub(crate) const PIPE_NONBLOCK: c_int = 2;

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

const _: () = assert!(MTYPE == 8, "QueueNode::mtype is the u64 heading the buffer");

/// The `N` bytes at `at` into the `EventData` after `out`'s mtype; zeros past its end.
fn head_bytes<const N: usize>(out: &[u64], at: usize) -> [u8; N] {
    core::array::from_fn(|i| {
        let byte = MTYPE + at + i;
        let word = out.get(byte / 8).map_or([0; 8], |word| word.to_ne_bytes());
        word.get(byte % 8).copied().unwrap_or(0)
    })
}

/// One hook's arguments and results. Only this crate makes and fills one, over
/// objects live for the hook: its pool and worker (null when none), its socket
/// (null or a live native socket) and `data`, which holds `length` bytes.
pub struct PoolCall<'a> {
    pub(crate) pool: crate::process_pool::PoolRef<'a>,
    pub(crate) worker: *mut c_void,
    pub(crate) socket: *mut c_void,
    pub(crate) data: *mut c_void,
    pub(crate) length: usize,
    pub(crate) flags: c_int,
    /// The call's return value.
    pub(crate) result: isize,
    /// `errno` right after the call.
    pub(crate) error: c_int,
}

impl<'a> PoolCall<'a> {
    pub(crate) fn new(pool: crate::process_pool::PoolRef<'a>, worker: *mut c_void) -> Self {
        Self {
            pool,
            worker,
            socket: ptr::null_mut(),
            data: ptr::null_mut(),
            length: 0,
            flags: 0,
            result: 0,
            error: 0,
        }
    }

    /// The live pool.
    #[must_use]
    pub fn pool(&self) -> crate::process_pool::PoolRef<'a> {
        self.pool
    }

    /// The live worker, if the call has one.
    #[must_use]
    pub fn worker(&self) -> Option<Record<'_>> {
        // SAFETY: only this crate makes a call, over null or one of its pool's live
        // workers, whose owner orders the writes made through the handle.
        unsafe { Record::from_raw(self.worker.cast()) }
    }

    /// The live native socket the hook works on, or null.
    #[must_use]
    pub fn socket(&self) -> *mut c_void {
        self.socket
    }

    /// The buffer of [`PoolCall::length`] bytes the hook reads or fills.
    #[must_use]
    pub fn data(&self) -> *mut c_void {
        self.data
    }

    #[must_use]
    pub fn length(&self) -> usize {
        self.length
    }

    /// The [`PoolCall::length`] bytes at [`PoolCall::data`] as they are now; none
    /// without a buffer. A hook that fills its buffer writes through `data`, with no
    /// borrow of this live.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        if self.data.is_null() {
            return &[];
        }
        // SAFETY: only this crate makes and fills a call: `data` is a buffer of at
        // least `length` initialized bytes (a zeroed receive buffer, or the bytes a
        // send lends) that outlives the hook, and so this borrow of its call.
        unsafe { std::slice::from_raw_parts(self.data.cast(), self.length) }
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

    /// The connection `accept` made: null, or a live native socket the pool now owns.
    ///
    /// # Safety
    /// `socket` is null or a live native socket, which the pool frees once.
    pub unsafe fn set_socket(&mut self, socket: *mut c_void) {
        self.socket = socket;
    }
}

/// What the bailout trap runs: a body over the argument the trap got with it.
pub type Step = unsafe extern "C" fn(call: *mut c_void);

/// The pool's IPC state. Native creates one per pool and frees it in `destroy`.
#[derive(Default)]
pub struct PoolIpc {
    /// `SW_IPC_SOCKET`: the listener (a native socket), its address, the
    /// connection being served and the reply the callback wrote.
    stream: Option<Stream>,
    /// The receive buffer, allocated when a worker's loop starts.
    packet: Vec<u64>,
}

struct Stream {
    socket: *mut c_void,
    file: Option<CString>,
    port: c_int,
    connection: *mut c_void,
    response: Option<Vec<u8>>,
}

/// `swr_zend_try`'s signature: `body` is never null.
type Trap = unsafe extern "C" fn(body: Option<Step>, arg: *mut c_void) -> c_int;
type Raise = unsafe extern "C" fn();

/// The extension's trap and raise, which [`set_bailout`] installs.
static BAILOUT: OnceLock<(Trap, Raise)> = OnceLock::new();

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

/// A body [`trapped`] runs, and what it returned.
struct Body<F, R> {
    body: Option<F>,
    result: Option<R>,
}

unsafe extern "C" fn step<F: FnOnce() -> R, R>(call: *mut c_void) {
    // SAFETY: `trapped` passes its body, which owns nothing a bailout skips.
    let body = unsafe { &mut *call.cast::<Body<F, R>>() };
    body.result = body.body.take().map(|body| body());
}

/// Runs `body` inside the bailout trap, when one is installed.
pub(crate) fn trapped<R, F: FnOnce() -> R>(body: F) -> Result<R, Bailed> {
    let Some(&(trap, _)) = BAILOUT.get() else {
        return Ok(body());
    };
    let mut body = Body {
        body: Some(body),
        result: None,
    };
    // SAFETY: the trap runs `step` over `body` inside `zend_try`; no Rust frame is
    // between its `setjmp` and `step`, whose body owns nothing a bailout skips.
    if unsafe { trap(Some(step::<F, R>), (&raw mut body).cast()) } != 0 {
        return Err(Bailed);
    }
    body.result.ok_or(Bailed)
}

/// One worker's loop over its pool.
struct Worker<'a> {
    native: &'a dyn PoolNative,
    /// Borrowed per use: the worker's callbacks reach the same handle, as
    /// `swr_v1_process_pool_ipc_respond` does.
    ipc: *mut PoolIpc,
    pool: crate::process_pool::PoolRef<'a>,
    worker: *mut c_void,
}

impl<'a> Worker<'a> {
    const fn ipc(&mut self) -> &mut PoolIpc {
        // SAFETY: `pool_run` found its live pool's IPC state there, not null: the
        // field is crate-private, written by `create` and cleared by `destroy`, which
        // no loop reaches. Borrowed until the next callback at most.
        unsafe { &mut *self.ipc }
    }

    fn call(&self) -> PoolCall<'a> {
        PoolCall::new(self.pool, self.worker)
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

    fn io(
        &self,
        hook: Hook,
        socket: *mut c_void,
        data: *mut u8,
        length: usize,
        flags: c_int,
    ) -> Result<PoolCall<'a>, Bailed> {
        let mut call = self.call();
        call.socket = socket;
        call.data = data.cast();
        call.length = length;
        call.flags = flags;
        run(self.native, hook, &mut call)?;
        Ok(call)
    }

    fn close(&self, socket: *mut c_void) {
        let mut call = self.call();
        call.socket = socket;
        self.native.close(&call);
    }

    /// Accept a connection: the connection, or the accept's `errno`.
    fn accept(&self) -> Result<Result<*mut c_void, c_int>, Bailed> {
        let mut call = self.call();
        run(self.native, |native, call| native.accept(call), &mut call)?;
        Ok(if call.socket.is_null() {
            Err(call.error)
        } else {
            Ok(call.socket)
        })
    }

    /// `socket->recv_sync(data, length, MSG_WAITALL)`.
    fn recv_all(&self, socket: *mut c_void, data: *mut u8, length: usize) -> Result<isize, Bailed> {
        Ok(self
            .io(
                |native, call| native.recv(call),
                socket,
                data,
                length,
                libc::MSG_WAITALL,
            )?
            .result)
    }

    fn send(&self, socket: *mut c_void, data: &[u8]) -> Result<isize, Bailed> {
        Ok(self
            .io(
                |native, call| native.send(call),
                socket,
                data.as_ptr().cast_mut(),
                data.len(),
                0,
            )?
            .result)
    }

    /// `Stream::recv_sync`: a length-prefixed frame of at most `length` bytes.
    fn recv_frame(
        &self,
        socket: *mut c_void,
        data: *mut u8,
        length: usize,
    ) -> Result<isize, Bailed> {
        let mut head = [0u8; 4];
        if self.recv_all(socket, head.as_mut_ptr(), head.len())? != 4 {
            return Ok(-1);
        }
        let size = i32::from_be_bytes(head);
        if size <= 0 || i64::from(size) > i64::try_from(length).unwrap_or(i64::MAX) {
            return Ok(-1);
        }
        let size = size.cast_unsigned() as usize;
        let n = self.recv_all(socket, data, size)?;
        Ok(if n == size.cast_signed() { n } else { -1 })
    }

    fn connection(&mut self) -> Option<&mut Stream> {
        self.ipc()
            .stream
            .as_mut()
            .filter(|stream| !stream.connection.is_null())
    }

    fn set_connection(&mut self, socket: *mut c_void) {
        if let Some(stream) = self.ipc().stream.as_mut() {
            stream.connection = socket;
        }
    }

    /// `SW_PROTOCOL_TASK`: one `EventData` per message.
    fn run_task(
        &mut self,
        mode: Mode,
        event_size: usize,
        by_sysvmsg: bool,
        worker_id: u32,
    ) -> Result<(), Bailed> {
        // `struct { long mtype; EventData buf; }`, zeroed.
        let mut out = vec![0u64; (MTYPE + event_size).div_ceil(8)];
        let Some(mtype) = out.first_mut() else {
            return Ok(());
        };
        *mtype = if by_sysvmsg {
            0
        } else {
            u64::from(worker_id) + 1
        };
        let server_fd = u64::from(worker_id.c_cast::<u16>());
        // `info.server_fd`: bytes 24..26 of the head, i.e. of out[4].
        if let Some(word) = out.get_mut(1 + HEAD_SERVER_FD / 8) {
            *word = server_fd << (8 * (HEAD_SERVER_FD % 8));
        }
        let mut buf = out.as_mut_ptr().cast::<u8>().wrapping_add(MTYPE);

        while self.running() {
            let n = match mode {
                Mode::Queue => {
                    // The received mtype stays in `out` for the next receive, as upstream.
                    let call = self.call();
                    let (n, error) = trapped(|| call.pool().queue_pop(&mut out, event_size))?;
                    // The receive borrowed `out`: its payload's address again.
                    buf = out.as_mut_ptr().cast::<u8>().wrapping_add(MTYPE);
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
                    let n = self.recv_frame(socket, buf, event_size)?;
                    if n <= 0 {
                        self.close(socket);
                        self.end()?;
                        continue;
                    }
                    self.set_connection(socket);
                    n
                }
                Mode::Pipe => {
                    let call = self.io(
                        |native, call| native.pipe_read(call),
                        ptr::null_mut(),
                        buf,
                        event_size,
                        0,
                    )?;
                    if call.result < 0 && fatal(call.error) {
                        self.warn(Warning::Read, call.error);
                        break;
                    }
                    call.result
                }
            };
            if n >= 0 {
                let length = u32::from_ne_bytes(head_bytes(&out, HEAD_LEN));
                let size = HEAD as u64 + u64::from(length);
                let tmpfile =
                    u16::from_ne_bytes(head_bytes(&out, HEAD_EXT_FLAGS)) & TASK_TMPFILE != 0;
                let alone = self.connection().is_none();
                if n.c_cast::<u64>() != size || (tmpfile && length != PACKET_TASK) {
                    // No task runs; a connection is still answered.
                    self.warn(
                        Warning::BadPacket {
                            received: n as i64,
                            length: size.cast_signed(),
                        },
                        0,
                    );
                } else {
                    // Without a connection to answer, nothing comes between the task
                    // and the iteration's end, so one trap covers both.
                    let mut call = self.call();
                    call.data = buf.cast();
                    call.length = event_size;
                    trapped(|| {
                        self.native.task(&mut call);
                        if call.result < 0 {
                            let fd = i64::from_ne_bytes(head_bytes(&out, 0));
                            self.warn(Warning::TaskFailed(fd), 0);
                        }
                        if alone {
                            self.native.end();
                        }
                    })?;
                    if alone {
                        continue;
                    }
                }
                if let Some(stream) = self.connection() {
                    let socket = std::mem::replace(&mut stream.connection, ptr::null_mut());
                    self.send(socket, &0i32.to_ne_bytes())?;
                    self.close(socket);
                }
            }
            self.end()?;
        }
        Ok(())
    }

    /// `SW_PROTOCOL_STREAM`: raw messages, length-prefixed over the stream socket.
    fn run_stream(&mut self, mode: Mode, max_packet_size: u32) -> Result<(), Bailed> {
        // A zeroed buffer: the `QueueNode` mtype is 0.
        let packet = self.ipc().prepare(max_packet_size);

        while self.running() {
            let (data, n) = match mode {
                Mode::Queue => {
                    let call = self.call();
                    let node = &mut self.ipc().packet;
                    let (n, error) = trapped(|| call.pool().queue_pop(node, MSGMAX))?;
                    if n < 0 && fatal(error) {
                        self.warn(Warning::Msgrcv, error);
                        break;
                    }
                    let node = &mut self.ipc().packet;
                    if let Some(mtype) = node.first_mut() {
                        *mtype = 0;
                    }
                    // The receive borrowed the packet: its payload's address again.
                    (node.as_mut_ptr().cast::<u8>().wrapping_add(MTYPE), n)
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
                    let mut received = self.recv_all(socket, head.as_mut_ptr(), head.len())? == 4;
                    let n = u32::from_be_bytes(head);
                    // Errors on one connection close it and carry on with the next.
                    received = received && n != 0 && n <= max_packet_size;
                    if !received
                        || self.recv_all(socket, packet, n as usize)? != n.c_cast::<isize>()
                    {
                        self.close(socket);
                        self.end()?;
                        continue;
                    }
                    self.set_connection(socket);
                    (packet, n.c_cast::<isize>())
                }
                Mode::Pipe => {
                    let call = self.io(
                        |native, call| native.pipe_read(call),
                        ptr::null_mut(),
                        packet,
                        max_packet_size as usize,
                        0,
                    )?;
                    if call.result < 0 && fatal(call.error) {
                        self.warn(Warning::Read, call.error);
                        break;
                    }
                    (packet, call.result)
                }
            };
            if n >= 0 {
                let length = n.c_cast::<u32>() as usize;
                self.io(
                    |native, call| native.message(call),
                    ptr::null_mut(),
                    data,
                    length,
                    0,
                )?;
                if let Some(stream) = self.connection() {
                    let socket = std::mem::replace(&mut stream.connection, ptr::null_mut());
                    let response = stream
                        .response
                        .as_mut()
                        .map(std::mem::take)
                        .unwrap_or_default();
                    if !response.is_empty() {
                        let head = response.len().c_cast::<u32>().to_be_bytes();
                        self.send(socket, &head)?;
                        self.send(socket, &response)?;
                    }
                    if let Some(stream) = self.ipc().stream.as_mut() {
                        // Keep the buffer's capacity, as `String::clear` does.
                        let mut response = response;
                        response.clear();
                        stream.response = Some(response);
                    }
                    self.close(socket);
                }
            }
            self.end()?;
        }
        Ok(())
    }

    /// `SW_PROTOCOL_MESSAGE`: the message bus over the worker's pipe.
    fn run_message(&mut self) -> Result<(), Bailed> {
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
    /// Allocate the packet buffer (and, for a stream listener, the response
    /// buffer) a worker receives into: its address.
    fn prepare(&mut self, max_packet_size: u32) -> *mut u8 {
        // A queue receive reads up to SW_MSGMAX bytes after the mtype.
        let size = (max_packet_size as usize).max(MTYPE + MSGMAX);
        self.packet = vec![0u64; size.div_ceil(8)];
        if let Some(stream) = self.stream.as_mut() {
            stream.response = Some(Vec::with_capacity(RESPONSE_CAPACITY));
        }
        self.packet.as_mut_ptr().cast()
    }
}

/// Install the extension's bailout trap (`swr_zend_try`) and its raise
/// (`zend_bailout`).
///
/// # Safety
/// `trap` runs its body inside `zend_try` and returns 1 when it bailed out;
/// `raise` calls `zend_bailout()`. The first pair installed stays.
pub unsafe fn set_bailout(trap: Trap, raise: Raise) {
    _ = BAILOUT.set((trap, raise));
}

/// Raise the bailout a loop returned [`BAILED`] for. Holds nothing: the
/// `longjmp` crosses only this frame.
pub fn raise_bailout() {
    if let Some(&(_, raise)) = BAILOUT.get() {
        // SAFETY: Zend's bailout, from a frame that owns nothing.
        unsafe { raise() };
    }
}

/// A pool's IPC state. `listener`: the pool uses a stream socket (`SW_IPC_SOCKET`).
#[must_use]
pub fn pool_ipc_new(listener: bool) -> *mut PoolIpc {
    let stream = listener.then_some(Stream {
        socket: ptr::null_mut(),
        file: None,
        port: 0,
        connection: ptr::null_mut(),
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
pub fn pool_ipc_free(ipc: PoolIpc, close: impl FnOnce(*mut c_void)) {
    if let Some(stream) = ipc.stream
        && !stream.socket.is_null()
    {
        if let Some(file) = &stream.file {
            swoole_rs_corebase::sys::unlink(file);
        }
        // The listener, freed once.
        close(stream.socket);
    }
}

/// Record a stream listener's address before it is created: false when the pool
/// has no stream socket.
#[cfg(any(feature = "upstream-symbols", test))]
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
/// `ProcessPool::dispatch(data)`: the same over a [`crate::stream::Stream`], which, as upstream's,
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
        let Some(mut stream) =
            crate::stream::Stream::create(&crate::stream::Peer::Unix(file.as_bytes()))
        else {
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

/// Keep the listener the native side created; `socket` is a native socket it now owns.
#[cfg(any(feature = "upstream-symbols", test))]
pub(crate) fn swr_v1_process_pool_ipc_set_listener(ipc: &mut PoolIpc, socket: *mut c_void) {
    if let Some(stream) = ipc.stream.as_mut() {
        stream.socket = socket;
    }
}

/// The stream listener, or null.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_process_pool_ipc_listener(ipc: &PoolIpc) -> *mut c_void {
    ipc.stream
        .as_ref()
        .map_or(ptr::null_mut(), |stream| stream.socket)
}

/// The listener's address and port, none without one.
#[cfg(test)]
pub(crate) fn pool_ipc_address(ipc: &PoolIpc) -> Option<(&CStr, c_int)> {
    let stream = ipc.stream.as_ref()?;
    Some((stream.file.as_deref()?, stream.port))
}

/// The connection being served (`stream_info_->last_connection`), or null.
#[must_use]
pub fn pool_ipc_connection(ipc: &PoolIpc) -> *mut c_void {
    ipc.stream
        .as_ref()
        .map_or(ptr::null_mut(), |stream| stream.connection)
}

/// `ProcessPool::response`: append to the reply; false (`SW_ERROR_INVALID_PARAMS`
/// for the caller) when there is nothing to send or no connection to reply to.
pub(crate) fn swr_v1_process_pool_ipc_respond(ipc: &mut PoolIpc, data: &[u8]) -> bool {
    let Some(stream) = ipc.stream.as_mut() else {
        return false;
    };
    if data.is_empty() || stream.connection.is_null() {
        return false;
    }
    let Some(response) = stream.response.as_mut() else {
        return false;
    };
    response.extend_from_slice(data);
    true
}

/// Allocate a worker's receive buffers for its event loop: the packet buffer.
pub fn pool_ipc_prepare(ipc: &mut PoolIpc, max_packet_size: u32) -> *mut c_char {
    ipc.prepare(max_packet_size).cast()
}

/// The packet buffer `_prepare` allocated.
pub fn pool_ipc_packet(ipc: &mut PoolIpc) -> *mut c_char {
    ipc.packet.as_mut_ptr().cast()
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
    pub pool: crate::process_pool::PoolRef<'a>,
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
    let ipc = pool_loop.pool.record().ipc;
    if ipc.is_null() {
        return Ok(());
    }
    let mut worker = Worker {
        native,
        ipc,
        pool: pool_loop.pool,
        worker: pool_loop.worker.raw().cast(),
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
    pool: crate::process_pool::PoolRef<'_>,
    pipe: *mut c_void,
    queue: bool,
    worker_id: u32,
    data: &[u8],
    flags: c_int,
) -> isize {
    let length = data.len();
    let mut call = PoolCall::new(pool, ptr::null_mut());
    if queue {
        let mut message = vec![0u64; (MTYPE + length).div_ceil(8)];
        if let Some(mtype) = message.first_mut() {
            *mtype = u64::from(worker_id) + 1;
        }
        let base = message.as_mut_ptr().cast::<u8>();
        // SAFETY: the words hold `MTYPE + length` bytes, so `data` fits after `mtype`.
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), base.wrapping_add(MTYPE), length) };
        // The queue's push, which cannot reach PHP.
        return if pool.queue_push(&message, length) {
            length.cast_signed()
        } else {
            -1
        };
    }
    if pipe.is_null() {
        // A worker without a pipe, which upstream doesn't send through.
        return -1;
    }
    // A worker's pipe end, a live socket (the record's invariant).
    call.socket = pipe;
    call.data = data.as_ptr().cast_mut().cast();
    call.length = length;
    call.flags = flags & PIPE_NONBLOCK;
    // The native pipe write, which cannot reach PHP.
    native.pipe_write(&mut call);
    set_errno(call.error);
    call.result
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
    pool: crate::process_pool::PoolRef<'_>,
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
        let call = PoolCall::new(pool, ptr::null_mut());
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
    let sent = swr_v1_process_worker_send(native, pool, pipe, queue, id, event, flags);
    if sent < 0 {
        let call = PoolCall::new(pool, ptr::null_mut());
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
            (*ipc).stream.as_mut().unwrap().connection = ptr::dangling_mut();
            assert!(!swr_v1_process_pool_ipc_respond(&mut *ipc, b""));
            assert!(swr_v1_process_pool_ipc_respond(&mut *ipc, b"ab"));
            assert_eq!(
                (*ipc).stream.as_ref().unwrap().response.as_deref(),
                Some(&b"ab"[..])
            );
            (*ipc).stream.as_mut().unwrap().connection = ptr::null_mut();
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
            assert!(swr_v1_process_pool_ipc_listener(&*ipc).is_null());
            pool_ipc_free(*Box::from_raw(ipc), |_| {});
        }
    }

    #[test]
    fn the_packet_buffer_holds_a_queue_message() {
        let mut ipc = PoolIpc::default();
        ipc.prepare(64);
        assert!(ipc.packet.len() * 8 >= MTYPE + MSGMAX);
        ipc.prepare(4 << 20);
        assert!(ipc.packet.len() * 8 >= 4 << 20);
    }

    fn records(busy: &[bool]) -> Vec<crate::worker_record::WorkerRecord> {
        busy.iter()
            .map(|&busy| {
                let mut record = *crate::worker_record::WorkerRecord::boxed();
                record.set_status(crate::worker_record::WorkerStatus::busy(busy));
                record
            })
            .collect()
    }

    fn schedule(
        workers: &mut [crate::worker_record::WorkerRecord],
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
        // SAFETY: the loop's buffer of `length` bytes.
        unsafe {
            ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                call.data.cast(),
                bytes.len().min(call.length),
            );
        }
        call.result = bytes.len().c_cast::<isize>();
    }

    fn bytes_of(call: &PoolCall) -> Vec<u8> {
        // SAFETY: the loop's bytes.
        unsafe { std::slice::from_raw_parts(call.data.cast::<u8>(), call.length) }.to_vec()
    }

    /// The scripted hooks, over the thread's [`Script`].
    struct Scripted;

    impl PoolNative for Scripted {
        fn call(&self, _: &PoolCall, _: crate::process_pool::Op, _: u64) -> i64 {
            0
        }
        fn alloc_workers(&self, _: u32) -> *mut crate::worker_record::WorkerRecord {
            core::ptr::null_mut()
        }
        fn new_message_bus(&self) -> Option<Box<dyn std::any::Any>> {
            None
        }
        fn callback(&self, _: &mut PoolCall, _: crate::process_pool::Callback) {}
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
            c.socket = next.map_or(ptr::null_mut(), |id| id as *mut c_void);
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
        fn close(&self, c: &PoolCall) {
            with(|s| s.closed.push(c.socket as usize));
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
            _: crate::process_pool::PoolRef<'_>,
            _: Record<'_>,
            _: &mut swoole_rs_corebase::DataHead,
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
        let mut pool = crate::process_pool::PoolRecord::empty();
        pool.ipc = ipc;
        let pool = std::cell::UnsafeCell::new(pool);
        let mut worker = crate::worker_record::WorkerRecord::empty();
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
        let mut pool = crate::process_pool::PoolRecord::empty();
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
