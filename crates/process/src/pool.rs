//! `ProcessPool`'s IPC: the state its workers receive through (the stream
//! listener's address and accepted connection, the response buffer, the packet
//! buffer) and the three worker loops over it (`SW_PROTOCOL_TASK`, `_STREAM` and
//! `_MESSAGE`), plus the master's side: picking an idle worker and sending it a
//! message. Rust owns the state, each loop's receive, framing, error policy and
//! reply. Native code lends its objects through [`PoolHooks`]: the queue's
//! `msgrcv`/`msgsnd`, the sockets' blocking calls (which wait and dispatch
//! signals), the message bus, the timers and the callbacks into the user's code.
//!
//! A hook that can reach PHP (a callback, a signal handler run while a socket
//! waits, the Zend allocator under the message bus) runs inside the trap the
//! extension installs with [`set_bailout`]. When PHP bails out
//! (`exit()`, a fatal error), the loop returns [`BAILED`] at once and its native
//! caller raises the bailout again with [`raise_bailout`], so the
//! `longjmp` never crosses a Rust frame.
#![expect(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "upstream's C conversions between ssize_t, uint32_t and long, kept so lengths and logged numbers match"
)]
use crate::worker_record::Worker as Record;
use std::ffi::{CStr, CString, c_char, c_int, c_long, c_void};
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use swoole_rs_corebase::sys::set_errno;

use crate::worker_record::WorkerRecord;

/// A loop's result: it ended, or PHP bailed out inside a hook.
pub const DONE: c_int = 0;
pub const BAILED: c_int = 1;

/// `SW_WORKER_IDLE`.
const WORKER_IDLE: u8 = 2;
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
const PIPE_NONBLOCK: c_int = 2;

/// Warnings native code formats (`PoolHooks::warn`); `errno` is the failed
/// call's where the message is a system warning.
pub const WARN_MSGRCV: c_int = 1;
pub const WARN_ACCEPT: c_int = 2;
pub const WARN_READ: c_int = 3;
pub const WARN_BAD_PACKET: c_int = 4;
pub const WARN_TASK_FAILED: c_int = 5;
pub const WARN_PIPE: c_int = 6;
pub const TRACE_POP: c_int = 7;
pub const WARN_SEND: c_int = 8;
pub const TRACE_PUSH: c_int = 9;

const _: () = assert!(MTYPE == 8, "QueueNode::mtype is the u64 heading the buffer");

/// The `T` at `at` bytes into `buf`.
fn read<T: Copy>(buf: *const u8, at: usize) -> T {
    // SAFETY: every caller reads a DataHead field inside its buffer.
    unsafe { buf.wrapping_add(at).cast::<T>().read_unaligned() }
}

/// One hook's arguments and results.
#[repr(C)]
pub struct PoolCall {
    pub pool: *mut c_void,
    pub worker: *mut c_void,
    pub socket: *mut c_void,
    pub data: *mut c_void,
    pub length: usize,
    pub flags: c_int,
    /// The call's return value.
    pub result: isize,
    /// `errno` right after the call.
    pub error: c_int,
}

impl PoolCall {
    pub(crate) fn new(pool: *mut c_void, worker: *mut c_void) -> Self {
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
}

/// A hook over a [`PoolCall`]; it may run inside the bailout trap.
pub type Step = unsafe extern "C" fn(call: *mut c_void);

/// The native pool's objects, one function per operation.
#[repr(C)]
pub struct PoolHooks {
    /// `ProcessPool::is_worker_running(worker)`.
    pub running: unsafe extern "C" fn(pool: *mut c_void, worker: *mut c_void) -> bool,
    /// Run the expired timers and dispatch the pending signals.
    pub end: Step,
    /// `queue->pop(data, length)`.
    pub queue_pop: Step,
    /// `queue->push(data, length)`: 1 when pushed.
    pub queue_push: Step,
    /// The stream listener's `accept()`: the connection in `socket`.
    pub accept: Step,
    /// `socket->recv_sync(data, length, flags)`.
    pub recv: Step,
    /// `socket->send_sync(data, length)`.
    pub send: Step,
    /// `socket->free()`.
    pub close: unsafe extern "C" fn(socket: *mut c_void),
    /// The worker's pipe, with the next timer as its read timeout: `read_sync`.
    pub pipe_read: Step,
    /// `pipe_worker->wait_event(next timer, SW_EVENT_READ)`.
    pub pipe_wait: Step,
    /// `message_bus->read(pipe_worker)`.
    pub bus_read: Step,
    /// `onMessage` with the bus's packet, then `message_bus->pop()`.
    pub bus_deliver: Step,
    /// `onTask(pool, worker, data)`.
    pub task: Step,
    /// `onMessage` with `length` bytes at `data`.
    pub message: Step,
    /// `worker->shutdown()`.
    pub shutdown: unsafe extern "C" fn(worker: *mut c_void),
    /// `socket` (a worker's pipe): `swoole_event_write` when the event loop is
    /// available and `flags` asks for it, else `send_sync`.
    pub pipe_write: Step,
    /// Report a `WARN_*` or `TRACE_*` with its numbers.
    pub warn:
        unsafe extern "C" fn(pool: *mut c_void, worker: *mut c_void, kind: c_int, a: i64, b: i64),
}

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

type Trap = unsafe extern "C" fn(body: Step, arg: *mut c_void) -> c_int;
type Raise = unsafe extern "C" fn();

static TRAP: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static RAISE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// The hook bailed out.
pub struct Bailed;

/// `catch_system_error`: whether `error` ends a loop.
fn fatal(error: c_int) -> bool {
    !matches!(error, 0 | libc::EAGAIN | libc::EINTR | libc::ETIMEDOUT)
}

/// Run native hook `step` over `call`.
fn invoke(step: Step, call: &mut PoolCall) {
    // SAFETY: a native hook over its call.
    unsafe { step(ptr::from_mut(call).cast()) };
}

/// Run `step` over `call`, inside the trap when one is installed.
pub(crate) fn run(step: Step, call: &mut PoolCall) -> Result<(), Bailed> {
    let trap = TRAP.load(Ordering::Acquire);
    if trap.is_null() {
        invoke(step, call);
        return Ok(());
    }
    let arg = ptr::from_mut(call).cast::<c_void>();
    // SAFETY: TRAP only ever holds a `Trap`.
    let trap = unsafe { std::mem::transmute::<*mut c_void, Trap>(trap) };
    // SAFETY: the trap runs the native hook inside `zend_try`; no Rust frame is
    // between its `setjmp` and the hook.
    if unsafe { trap(step, arg) } == 0 {
        Ok(())
    } else {
        Err(Bailed)
    }
}

/// A body [`trapped`] runs, and what it returned.
struct Body<F, R> {
    body: Option<F>,
    result: Option<R>,
}

unsafe extern "C" fn step<F: FnOnce() -> R, R>(call: *mut c_void) {
    // SAFETY: `trapped` passes its call.
    let data = unsafe { (*call.cast::<PoolCall>()).data };
    // SAFETY: the call's data is `trapped`'s body, which owns nothing a bailout skips.
    let body = unsafe { &mut *data.cast::<Body<F, R>>() };
    body.result = body.body.take().map(|body| body());
}

/// Runs `body` inside the bailout trap.
pub(crate) fn trapped<R, F: FnOnce() -> R>(body: F) -> Result<R, Bailed> {
    let mut body = Body {
        body: Some(body),
        result: None,
    };
    let mut call = PoolCall::new(ptr::null_mut(), ptr::null_mut());
    call.data = (&raw mut body).cast();
    run(step::<F, R>, &mut call)?;
    body.result.ok_or(Bailed)
}

/// One worker's loop over its pool.
struct Worker<'a> {
    hooks: &'a PoolHooks,
    /// Borrowed per use: the worker's callbacks reach the same handle, as
    /// `swr_v1_process_pool_ipc_respond` does.
    ipc: *mut PoolIpc,
    pool: *mut c_void,
    worker: *mut c_void,
}

impl Worker<'_> {
    const fn ipc(&mut self) -> &mut PoolIpc {
        // SAFETY: `pool_run`'s live handle, borrowed until the next callback at most.
        unsafe { &mut *self.ipc }
    }

    fn call(&self) -> PoolCall {
        PoolCall::new(self.pool, self.worker)
    }

    fn running(&self) -> bool {
        // SAFETY: the host's pool and worker.
        unsafe { (self.hooks.running)(self.pool, self.worker) }
    }

    fn warn(&self, kind: c_int, error: c_int, a: i64, b: i64) {
        set_errno(error);
        // SAFETY: as above.
        unsafe { (self.hooks.warn)(self.pool, self.worker, kind, a, b) };
    }

    fn end(&self) -> Result<(), Bailed> {
        run(self.hooks.end, &mut self.call())
    }

    fn io(
        &self,
        step: Step,
        socket: *mut c_void,
        data: *mut u8,
        length: usize,
        flags: c_int,
    ) -> Result<PoolCall, Bailed> {
        let mut call = self.call();
        call.socket = socket;
        call.data = data.cast();
        call.length = length;
        call.flags = flags;
        run(step, &mut call)?;
        Ok(call)
    }

    fn close(&self, socket: *mut c_void) {
        // SAFETY: a connection the listener accepted, closed once.
        unsafe { (self.hooks.close)(socket) };
    }

    /// Accept a connection: the connection, or the accept's `errno`.
    fn accept(&self) -> Result<Result<*mut c_void, c_int>, Bailed> {
        let mut call = self.call();
        run(self.hooks.accept, &mut call)?;
        Ok(if call.socket.is_null() {
            Err(call.error)
        } else {
            Ok(call.socket)
        })
    }

    /// `socket->recv_sync(data, length, MSG_WAITALL)`.
    fn recv_all(&self, socket: *mut c_void, data: *mut u8, length: usize) -> Result<isize, Bailed> {
        Ok(self
            .io(self.hooks.recv, socket, data, length, libc::MSG_WAITALL)?
            .result)
    }

    fn send(&self, socket: *mut c_void, data: &[u8]) -> Result<isize, Bailed> {
        Ok(self
            .io(
                self.hooks.send,
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
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream stores the id in the u16 server_fd"
        )]
        let server_fd = u64::from(worker_id as u16);
        // `info.server_fd`: bytes 24..26 of the head, i.e. of out[4].
        if let Some(word) = out.get_mut(1 + HEAD_SERVER_FD / 8) {
            *word = server_fd << (8 * (HEAD_SERVER_FD % 8));
        }
        let base = out.as_mut_ptr().cast::<u8>();
        let buf = base.wrapping_add(MTYPE);

        while self.running() {
            let n = match mode {
                Mode::Queue => {
                    // The received mtype stays in `out` for the next receive, as upstream.
                    let call =
                        self.io(self.hooks.queue_pop, ptr::null_mut(), base, event_size, 0)?;
                    if call.result < 0 && fatal(call.error) {
                        self.warn(WARN_MSGRCV, call.error, 0, 0);
                        break;
                    }
                    call.result
                }
                Mode::Socket => {
                    let socket = match self.accept()? {
                        Ok(socket) => socket,
                        Err(error) if fatal(error) => {
                            self.warn(WARN_ACCEPT, error, 0, 0);
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
                    let call =
                        self.io(self.hooks.pipe_read, ptr::null_mut(), buf, event_size, 0)?;
                    if call.result < 0 && fatal(call.error) {
                        self.warn(WARN_READ, call.error, 0, 0);
                        break;
                    }
                    call.result
                }
            };
            if n >= 0 {
                let length = read::<u32>(buf, HEAD_LEN);
                let size = HEAD as u64 + u64::from(length);
                let tmpfile = read::<u16>(buf, HEAD_EXT_FLAGS) & TASK_TMPFILE != 0;
                let alone = self.connection().is_none();
                if n as u64 != size || (tmpfile && length != PACKET_TASK) {
                    // No task runs; a connection is still answered.
                    self.warn(WARN_BAD_PACKET, 0, n as i64, size.cast_signed());
                } else {
                    // Without a connection to answer, nothing comes between the task
                    // and the iteration's end, so one trap covers both.
                    let mut call = self.call();
                    call.data = buf.cast();
                    call.length = event_size;
                    trapped(|| {
                        invoke(self.hooks.task, &mut call);
                        if call.result < 0 {
                            self.warn(WARN_TASK_FAILED, 0, read::<i64>(buf, 0), 0);
                        }
                        if alone {
                            invoke(self.hooks.end, &mut call);
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
                    let call = self.io(self.hooks.queue_pop, ptr::null_mut(), packet, MSGMAX, 0)?;
                    if call.result < 0 && fatal(call.error) {
                        self.warn(WARN_MSGRCV, call.error, 0, 0);
                        break;
                    }
                    self.warn(TRACE_POP, 0, call.result as i64, 0);
                    if let Some(mtype) = self.ipc().packet.first_mut() {
                        *mtype = 0;
                    }
                    (packet.wrapping_add(MTYPE), call.result)
                }
                Mode::Socket => {
                    let socket = match self.accept()? {
                        Ok(socket) => socket,
                        Err(error) if fatal(error) => {
                            self.warn(WARN_ACCEPT, error, 0, 0);
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
                    if !received || self.recv_all(socket, packet, n as usize)? != n as isize {
                        self.close(socket);
                        self.end()?;
                        continue;
                    }
                    self.set_connection(socket);
                    (packet, n as isize)
                }
                Mode::Pipe => {
                    let call = self.io(
                        self.hooks.pipe_read,
                        ptr::null_mut(),
                        packet,
                        max_packet_size as usize,
                        0,
                    )?;
                    if call.result < 0 && fatal(call.error) {
                        self.warn(WARN_READ, call.error, 0, 0);
                        break;
                    }
                    (packet, call.result)
                }
            };
            if n >= 0 {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "upstream stores the length in the u32 info.len"
                )]
                let length = n as u32 as usize;
                self.io(self.hooks.message, ptr::null_mut(), data, length, 0)?;
                if let Some(stream) = self.connection() {
                    let socket = std::mem::replace(&mut stream.connection, ptr::null_mut());
                    let response = stream
                        .response
                        .as_mut()
                        .map(std::mem::take)
                        .unwrap_or_default();
                    if !response.is_empty() {
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "upstream's htonl of the length"
                        )]
                        let head = (response.len() as u32).to_be_bytes();
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
                    self.warn(WARN_PIPE, error, 0, 0);
                    // SAFETY: the host's worker.
                    unsafe { (self.hooks.shutdown)(self.worker) };
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
            run(self.hooks.pipe_wait, &mut call)?;
            if call.result < 0 {
                return Ok(if fatal(call.error) {
                    Received::Error(call.error)
                } else {
                    Received::Continue
                });
            }
            let mut call = self.call();
            run(self.hooks.bus_read, &mut call)?;
            match call.result {
                n if n < 0 => return Ok(interrupted(call.error)),
                0 => {}
                _ => {
                    run(self.hooks.bus_deliver, &mut self.call())?;
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
/// `raise` calls `zend_bailout()`.
pub unsafe fn set_bailout(trap: Trap, raise: Raise) {
    TRAP.store(trap as *mut c_void, Ordering::Release);
    RAISE.store(raise as *mut c_void, Ordering::Release);
}

/// Raise the bailout a loop returned [`BAILED`] for. Holds nothing: the
/// `longjmp` crosses only this frame.
pub fn raise_bailout() {
    let raise = RAISE.load(Ordering::Acquire);
    if !raise.is_null() {
        // SAFETY: RAISE only ever holds a `Raise`.
        let raise = unsafe { std::mem::transmute::<*mut c_void, Raise>(raise) };
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

/// Free the state; `close` frees the listener. A listener's address is a socket
/// file, unlinked as upstream does (a TCP host is unlinked too, harmlessly).
///
/// # Safety
/// `ipc` is null or a live handle, not used afterwards; `close` frees a native socket.
pub unsafe fn pool_ipc_free(ipc: *mut PoolIpc, close: unsafe extern "C" fn(*mut c_void)) {
    if ipc.is_null() {
        return;
    }
    // SAFETY: the caller hands back the handle `_new` leaked.
    let ipc = unsafe { Box::from_raw(ipc) };
    if let Some(stream) = ipc.stream
        && !stream.socket.is_null()
    {
        if let Some(file) = &stream.file {
            // SAFETY: a NUL-terminated path.
            unsafe { libc::unlink(file.as_ptr()) };
        }
        // SAFETY: the listener, freed once.
        unsafe { close(stream.socket) };
    }
}

/// Record a stream listener's address before it is created: false when the pool
/// has no stream socket.
///
/// # Safety
/// `ipc` is a live handle; `host` is a NUL-terminated string.
pub(crate) unsafe fn swr_v1_process_pool_ipc_address(
    ipc: *mut PoolIpc,
    host: *const c_char,
    port: c_int,
) -> bool {
    // SAFETY: the caller's live handle.
    let Some(stream) = (unsafe { &mut *ipc }).stream.as_mut() else {
        return false;
    };
    // SAFETY: the caller's string.
    stream.file = Some(unsafe { CStr::from_ptr(host) }.to_owned());
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
///
/// # Safety
/// `data` holds `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_pool_ipc_send_sync(
    ipc: Option<&PoolIpc>,
    data: *const u8,
    len: u32,
) -> bool {
    // SAFETY: the caller's `len` bytes.
    let data = unsafe { std::slice::from_raw_parts(data, len as usize) };
    swr_v1_process_pool_ipc_send(ipc, data, true)
}

/// Keep the listener the native side created; `socket` is a native socket it now owns.
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

/// The listener's address (null without one) and port.
///
/// # Safety
/// `ipc` is a live handle; `port` is writable.
#[cfg(test)]
pub(crate) unsafe fn swr_v1_process_pool_ipc_address_of(
    ipc: *const PoolIpc,
    port: *mut c_int,
) -> *const c_char {
    // SAFETY: the caller's live handle.
    let Some(stream) = (unsafe { &*ipc }).stream.as_ref() else {
        return ptr::null();
    };
    // SAFETY: the caller's out-pointer.
    unsafe { *port = stream.port };
    stream
        .file
        .as_ref()
        .map_or(ptr::null(), |file| file.as_ptr())
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
///
/// # Safety
/// `ipc` is a live handle; `data` is null or `length` readable bytes.
pub(crate) unsafe fn swr_v1_process_pool_ipc_respond(
    ipc: *mut PoolIpc,
    data: *const u8,
    length: u32,
) -> bool {
    // SAFETY: the caller's live handle.
    let Some(stream) = (unsafe { &mut *ipc }).stream.as_mut() else {
        return false;
    };
    if data.is_null() || length == 0 || stream.connection.is_null() {
        return false;
    }
    let Some(response) = stream.response.as_mut() else {
        return false;
    };
    // SAFETY: the caller's bytes.
    response.extend_from_slice(unsafe { std::slice::from_raw_parts(data, length as usize) });
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

/// `SW_IPC_UNIXSOCK`/`_MSGQUEUE`/`_SOCKET`, as `ProcessPool::ipc_mode`.
const IPC_MSGQUEUE: u8 = 2;
const IPC_SOCKET: u8 = 3;

fn mode(ipc_mode: u8) -> Mode {
    match ipc_mode {
        IPC_MSGQUEUE => Mode::Queue,
        IPC_SOCKET => Mode::Socket,
        _ => Mode::Pipe,
    }
}

/// What a worker loop needs to know about its pool.
#[repr(C)]
pub struct PoolLoop {
    pub hooks: *const PoolHooks,
    pub ipc: *mut PoolIpc,
    pub pool: *mut c_void,
    pub worker: *mut c_void,
    pub worker_id: u32,
    /// `use_msgqueue`/`use_socket` as an IPC mode (`SW_IPC_*`).
    pub ipc_mode: u8,
    pub schedule_by_sysvmsg: bool,
    pub max_packet_size: u32,
    /// `sizeof(EventData)`.
    pub event_size: usize,
}

/// `ProcessPool::run_with_task_protocol`/`_stream_protocol`/`_message_protocol`
/// (`protocol` is `SW_PROTOCOL_*`), between `at_worker_enter` and
/// `at_worker_exit`: [`DONE`], or [`BAILED`] to raise at once.
///
/// # Safety
/// `pool_loop` describes a live pool, worker and hooks for this call.
#[must_use]
pub unsafe fn pool_run(pool_loop: *const PoolLoop, protocol: c_int) -> c_int {
    // SAFETY: the caller's description.
    let pool_loop = unsafe { &*pool_loop };
    let mut worker = Worker {
        // SAFETY: the caller's hooks and state, for this call.
        hooks: unsafe { &*pool_loop.hooks },
        ipc: pool_loop.ipc,
        pool: pool_loop.pool,
        worker: pool_loop.worker,
    };
    let mode = mode(pool_loop.ipc_mode);
    let result = match protocol {
        1 => worker.run_task(
            mode,
            pool_loop.event_size,
            pool_loop.schedule_by_sysvmsg,
            pool_loop.worker_id,
        ),
        2 => worker.run_stream(mode, pool_loop.max_packet_size),
        _ => worker.run_message(),
    };
    if result.is_ok() { DONE } else { BAILED }
}

/// `ProcessPool::schedule`'s round robin: the first idle worker from the next
/// round, or the last one tried (and `warning` set) when all are busy.
///
/// # Safety
/// `workers` holds `count` (> 0) live worker records; `round_id` is the pool's
/// shared counter and `warning` its flag.
pub unsafe fn pool_schedule(
    workers: *const WorkerRecord,
    count: u32,
    round_id: *mut u32,
    warning: *mut u8,
) -> u32 {
    // SAFETY: the pool's counter, shared with other processes and updated atomically.
    let round = unsafe { AtomicU32::from_ptr(round_id) };
    let mut target = 0;
    for _ in 0..=count {
        target = round.fetch_add(1, Ordering::SeqCst) % count;
        // SAFETY: `target` < `count`.
        if unsafe { Record::new(workers.wrapping_add(target as usize)).status() } == WORKER_IDLE {
            return target;
        }
    }
    // SAFETY: the pool's flag.
    unsafe { *warning = 1 };
    target
}

/// `Worker::send_pipe_message`: through the pool's queue (as `mtype` worker id + 1)
/// when it has one, else the master's or the worker's end of the pipe.
///
/// # Safety
/// `hooks` and the pool are live; `pipe` is the chosen pipe end (unused with a
/// queue); `data` holds `length` bytes, at most `sizeof(EventData)` with a queue.
pub unsafe extern "C" fn swr_v1_process_worker_send(
    hooks: *const PoolHooks,
    pool: *mut c_void,
    pipe: *mut c_void,
    queue: bool,
    worker_id: u32,
    data: *const u8,
    length: usize,
    flags: c_int,
) -> isize {
    // SAFETY: the caller's hooks.
    let hooks = unsafe { &*hooks };
    let mut call = PoolCall::new(pool, ptr::null_mut());
    if queue {
        let mut message = vec![0u64; (MTYPE + length).div_ceil(8)];
        if let Some(mtype) = message.first_mut() {
            *mtype = u64::from(worker_id) + 1;
        }
        let base = message.as_mut_ptr().cast::<u8>();
        // SAFETY: the caller's bytes fit after `mtype`.
        unsafe { ptr::copy_nonoverlapping(data, base.wrapping_add(MTYPE), length) };
        // SAFETY: the host's pool.
        unsafe { (hooks.warn)(pool, ptr::null_mut(), TRACE_PUSH, length as i64, 0) };
        call.data = base.cast();
        call.length = length;
        // SAFETY: the native queue's push, which cannot reach PHP.
        unsafe { (hooks.queue_push)(ptr::from_mut(&mut call).cast()) };
        return if call.result != 0 {
            length.cast_signed()
        } else {
            -1
        };
    }
    call.socket = pipe;
    call.data = data.cast_mut().cast();
    call.length = length;
    call.flags = flags & PIPE_NONBLOCK;
    // SAFETY: the native pipe write, which cannot reach PHP.
    unsafe { (hooks.pipe_write)(ptr::from_mut(&mut call).cast()) };
    set_errno(call.error);
    call.result
}

/// `ProcessPool::dispatch`/`dispatch_sync` to a worker's pipe or queue: choose
/// the worker when `*worker_id` is negative, offset it by `start_id`, and send
/// the event (`blocking`: wait for the pipe). False after reporting a failed send.
///
/// # Safety
/// As [`swr_v1_process_worker_send`]; `workers` holds `count` live records,
/// `worker_id` is writable, `event` holds `size` bytes.
#[expect(
    clippy::too_many_arguments,
    reason = "the flat list of the former C export; `Pool::dispatch` is the one caller"
)]
pub unsafe fn pool_dispatch(
    hooks: *const PoolHooks,
    pool: *mut c_void,
    workers: *mut WorkerRecord,
    count: u32,
    start_id: u16,
    round_id: *mut u32,
    warning: *mut u8,
    schedule_by_sysvmsg: bool,
    queue: bool,
    worker_id: *mut c_int,
    event: *const u8,
    size: u32,
    blocking: bool,
) -> bool {
    // SAFETY: the caller's out-pointer.
    let target = unsafe { &mut *worker_id };
    if *target < 0 {
        *target = if schedule_by_sysvmsg {
            0
        } else {
            // SAFETY: as the caller guarantees.
            unsafe { pool_schedule(workers, count, round_id, warning) }.cast_signed()
        };
    }
    *target += c_int::from(start_id);
    let index = (*target - c_int::from(start_id)).cast_unsigned() as usize;
    let worker = workers.wrapping_add(index);
    // SAFETY: the chosen worker's record.
    let id = unsafe { Record::new(worker).id() };
    // SAFETY: as above.
    let pipe = unsafe { Record::new(worker).pipe_master() };
    let flags = PIPE_MASTER | if blocking { 0 } else { PIPE_NONBLOCK };
    // SAFETY: as the caller guarantees.
    let sent = unsafe {
        swr_v1_process_worker_send(hooks, pool, pipe, queue, id, event, size as usize, flags)
    };
    if sent < 0 {
        // SAFETY: the caller's hooks.
        let warn = unsafe { (*hooks).warn };
        // SAFETY: the host's pool.
        unsafe {
            warn(
                pool,
                ptr::null_mut(),
                WARN_SEND,
                i64::from(size),
                i64::from(*target),
            );
        }
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn noop(_: *mut c_void) {}

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
            assert!(!swr_v1_process_pool_ipc_respond(ipc, b"x".as_ptr(), 1));
            pool_ipc_prepare(&mut *ipc, 16);
            assert!(!swr_v1_process_pool_ipc_respond(ipc, b"x".as_ptr(), 1));
            (*ipc).stream.as_mut().unwrap().connection = ptr::dangling_mut();
            assert!(!swr_v1_process_pool_ipc_respond(ipc, ptr::null(), 1));
            assert!(!swr_v1_process_pool_ipc_respond(ipc, b"x".as_ptr(), 0));
            assert!(swr_v1_process_pool_ipc_respond(ipc, b"ab".as_ptr(), 2));
            assert_eq!(
                (*ipc).stream.as_ref().unwrap().response.as_deref(),
                Some(&b"ab"[..])
            );
            (*ipc).stream.as_mut().unwrap().connection = ptr::null_mut();
            pool_ipc_free(ipc, noop);
        }
    }

    #[test]
    fn a_pool_without_a_listener_has_no_address() {
        let ipc = pool_ipc_new(false);
        let mut port = 7;
        // SAFETY: a live handle.
        unsafe {
            assert!(!swr_v1_process_pool_ipc_address(ipc, c"/tmp/x".as_ptr(), 1));
            assert!(swr_v1_process_pool_ipc_address_of(ipc, &raw mut port).is_null());
            assert!(swr_v1_process_pool_ipc_listener(&*ipc).is_null());
            pool_ipc_free(ipc, noop);
        }
        assert_eq!(port, 7);
    }

    #[test]
    fn the_packet_buffer_holds_a_queue_message() {
        let mut ipc = PoolIpc::default();
        ipc.prepare(64);
        assert!(ipc.packet.len() * 8 >= MTYPE + MSGMAX);
        ipc.prepare(4 << 20);
        assert!(ipc.packet.len() * 8 >= 4 << 20);
    }

    fn records(statuses: &[u8]) -> Vec<WorkerRecord> {
        statuses
            .iter()
            .map(|&status| {
                // SAFETY: a record is plain data; zero is a valid value.
                let mut record: WorkerRecord = unsafe { std::mem::zeroed() };
                // SAFETY: a local record.
                unsafe { Record::new(&raw mut record).set_status(status) };
                record
            })
            .collect()
    }

    #[test]
    fn schedule_picks_the_next_idle_worker() {
        let workers = records(&[1, 2, 1]);
        let mut round = 0;
        let mut warning = 0;
        // SAFETY: local records and counters.
        let target =
            unsafe { pool_schedule(workers.as_ptr(), 3, &raw mut round, &raw mut warning) };
        assert_eq!((target, round, warning), (1, 2, 0));
    }

    #[test]
    fn schedule_warns_when_every_worker_is_busy() {
        let workers = records(&[1, 1]);
        let mut round = 5;
        let mut warning = 0;
        // SAFETY: local records and counters.
        let target =
            unsafe { pool_schedule(workers.as_ptr(), 2, &raw mut round, &raw mut warning) };
        // Three tries (count + 1): rounds 5, 6, 7; the last is 7 % 2.
        assert_eq!((target, round, warning), (1, 8, 1));
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
        warnings: Vec<(c_int, i64, i64)>,
        respond: Vec<u8>,
        ipc: usize,
    }

    thread_local! {
        static SCRIPT: std::cell::RefCell<Script> = std::cell::RefCell::default();
    }

    fn with<R>(f: impl FnOnce(&mut Script) -> R) -> R {
        SCRIPT.with(|script| f(&mut script.borrow_mut()))
    }

    fn call(arg: *mut c_void) -> &'static mut PoolCall {
        // SAFETY: the loop's call.
        unsafe { &mut *arg.cast::<PoolCall>() }
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
        call.result = bytes.len() as isize;
    }

    fn bytes_of(call: &PoolCall) -> Vec<u8> {
        // SAFETY: the loop's bytes.
        unsafe { std::slice::from_raw_parts(call.data.cast::<u8>(), call.length) }.to_vec()
    }

    unsafe extern "C" fn running(_: *mut c_void, _: *mut c_void) -> bool {
        with(|s| {
            s.rounds = s.rounds.saturating_sub(1);
            s.rounds > 0
        })
    }
    unsafe extern "C" fn end(_: *mut c_void) {
        with(|s| s.ends += 1);
    }
    unsafe extern "C" fn accept(arg: *mut c_void) {
        let next = with(|s| s.accepts.pop());
        let c = call(arg);
        c.socket = next.map_or(ptr::null_mut(), |id| id as *mut c_void);
        c.error = libc::EAGAIN;
    }
    unsafe extern "C" fn recv(arg: *mut c_void) {
        let next = with(|s| s.recvs.remove(0));
        fill(call(arg), &next);
    }
    unsafe extern "C" fn send(arg: *mut c_void) {
        let c = call(arg);
        let bytes = bytes_of(c);
        c.result = bytes.len() as isize;
        with(|s| s.sent.push(bytes));
    }
    unsafe extern "C" fn close(socket: *mut c_void) {
        with(|s| s.closed.push(socket as usize));
    }
    unsafe extern "C" fn pipe_read(arg: *mut c_void) {
        let (bytes, error) = with(|s| s.reads.remove(0));
        let c = call(arg);
        if error == 0 {
            fill(c, &bytes);
        } else {
            c.result = -1;
            c.error = error;
        }
    }
    unsafe extern "C" fn pipe_wait(arg: *mut c_void) {
        call(arg).result = 0;
    }
    unsafe extern "C" fn bus_read(arg: *mut c_void) {
        let (result, error) = with(|s| s.bus.remove(0));
        let c = call(arg);
        c.result = result;
        c.error = error;
    }
    unsafe extern "C" fn bus_deliver(_: *mut c_void) {
        with(|s| s.delivered += 1);
    }
    unsafe extern "C" fn task(arg: *mut c_void) {
        call(arg).result = with(|s| {
            s.tasks += 1;
            s.task_result
        });
    }
    unsafe extern "C" fn message(arg: *mut c_void) {
        let bytes = bytes_of(call(arg));
        let (ipc, respond) = with(|s| {
            s.messages.push(bytes);
            (s.ipc as *mut PoolIpc, std::mem::take(&mut s.respond))
        });
        if !respond.is_empty() {
            // SAFETY: the test's handle.
            let responded = unsafe {
                swr_v1_process_pool_ipc_respond(
                    ipc,
                    respond.as_ptr(),
                    u32::try_from(respond.len()).unwrap(),
                )
            };
            assert!(responded);
        }
    }
    unsafe extern "C" fn shutdown(_: *mut c_void) {
        with(|s| s.shutdowns += 1);
    }
    unsafe extern "C" fn warn(_: *mut c_void, _: *mut c_void, kind: c_int, a: i64, b: i64) {
        with(|s| s.warnings.push((kind, a, b)));
    }

    static HOOKS: PoolHooks = PoolHooks {
        running,
        end,
        queue_pop: noop,
        queue_push: noop,
        accept,
        recv,
        send,
        close,
        pipe_read,
        pipe_wait,
        bus_read,
        bus_deliver,
        task,
        message,
        shutdown,
        pipe_write: noop,
        warn,
    };

    fn run_loop(script: Script, listener: bool, ipc_mode: u8, protocol: c_int) -> Script {
        let ipc = pool_ipc_new(listener);
        with(|s| {
            *s = script;
            s.ipc = ipc as usize;
        });
        let pool_loop = PoolLoop {
            hooks: &raw const HOOKS,
            ipc,
            pool: ptr::null_mut(),
            worker: ptr::null_mut(),
            worker_id: 3,
            ipc_mode,
            schedule_by_sysvmsg: false,
            max_packet_size: 64,
            event_size: 128,
        };
        // SAFETY: the scripted hooks and a live handle.
        assert_eq!(unsafe { pool_run(&raw const pool_loop, protocol) }, DONE);
        // SAFETY: as above.
        unsafe { pool_ipc_free(ipc, noop) };
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
            IPC_SOCKET,
            2,
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
            IPC_SOCKET,
            2,
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
            1,
            1,
        );
        // Neither bad packet reaches the task.
        assert_eq!(script.tasks, 1);
        assert_eq!(
            script.warnings,
            vec![
                (WARN_BAD_PACKET, HEAD as i64 + 2, HEAD as i64 + 4),
                (WARN_BAD_PACKET, HEAD as i64 + 4, HEAD as i64 + 4),
                (WARN_TASK_FAILED, 42, 0),
                (WARN_READ, 0, 0),
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
            1,
            3,
        );
        assert_eq!(script.delivered, 1);
        // The delivered message and the interrupted read each end an iteration.
        assert_eq!(script.ends, 2);
        assert_eq!(script.shutdowns, 1);
        assert_eq!(script.warnings, vec![(WARN_PIPE, 0, 0)]);
    }
}
