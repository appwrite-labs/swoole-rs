//! `Protocol::recv_with_length_protocol`, `recv_with_eof_protocol` and
//! `recv_split_by_eof`, each one call. The length loop reads greedily: one `recv` of
//! the buffer's free space, then every whole package in it, where upstream reads the
//! length header and then exactly the rest of the package, a `recv` each. The receive buffer (`swoole::String`) and
//! the protocol's leading fields are read and written in place; the caller performs
//! each socket, callback, allocation or log effect through [`Effects::effect`].
use core::ffi::c_void;
use core::ptr::addr_of;

/// The socket, buffer and protocol effects of one receive.
pub trait Effects {
    /// Performs `op`: applies [`FLAG_WAIT`] from `flags` to the socket first, and
    /// refreshes `flags` from the socket after.
    fn effect(&mut self, op: u32, arg: u64, flags: &mut u32) -> i64;
}

pub const MODE_LENGTH: u32 = 0;
pub const MODE_EOF: u32 = 1;
pub const MODE_SPLIT: u32 = 2;

/// `socket->recv` of arg bytes into the buffer's tail, with upstream's read-error
/// handling and warning: the byte count, or `RECV_CLOSE`/`RECV_DONE`.
pub const OP_RECV: u32 = 1;
/// `get_package_length` over the whole buffer.
pub const OP_GET_LENGTH: u32 = 2;
pub const OP_EXTEND: u32 = 3;
/// `onPackage` of arg bytes from the buffer start.
pub const OP_DISPATCH: u32 = 4;
pub const OP_REDUCE: u32 = 5;
/// `buffer->split` on the EOF, each part to `onPackage`.
pub const OP_SPLIT: u32 = 7;
pub const OP_SSL_PENDING: u32 = 8;
/// `socket->skip_recv = 0`.
pub const OP_CLEAR_SKIP: u32 = 9;
/// `swoole_pagesize()`.
pub const OP_PAGE: u32 = 10;
pub const OP_LOG_MALFORMED: u32 = 11;
pub const OP_LOG_NO_LENGTH: u32 = 12;
/// arg: the packet length (length protocol).
pub const OP_LOG_TOO_LARGE: u32 = 13;
/// Make the close that follows a reset (`SO_LINGER` 0): the length loop holds bytes
/// past a bad package's header that upstream, reading only the header, would have
/// left unread, and closing over unread bytes resets.
pub const OP_RESET: u32 = 14;

/// `OP_RECV`: the connection closes (`SW_ERR`).
pub const RECV_CLOSE: i64 = -1;
/// `OP_RECV`: the read ends with `SW_OK` (would block, logged error, removed socket).
pub const RECV_DONE: i64 = -2;
/// Receive flag bits: `socket->removed`, `recv_wait`, `ssl != nullptr`, and, at
/// entry only, `skip_recv` and a dynamic `get_package_length_size`. The loop sets
/// `recv_wait` here; the caller applies it before each effect and on return.
/// [`FLAG_PAUSED`] goes with `FLAG_REMOVED` when the socket left the reactor only to
/// stop reading (`pause`) and its connection stays open.
pub const FLAG_REMOVED: u32 = 1;
pub const FLAG_WAIT: u32 = 2;
pub const FLAG_TLS: u32 = 4;
pub const FLAG_SKIP: u32 = 8;
pub const FLAG_DYNAMIC: u32 = 16;
pub const FLAG_PAUSED: u32 = 32;

const SW_OK: i32 = 0;
const SW_ERR: i32 = -1;
const SW_CONTINUE: i32 = 1;
const SW_CLOSE: i32 = 3;
const BUFFER_SIZE_STD: usize = 8192;

/// The head of `swoole::String`.
#[repr(C)]
pub struct Buffer {
    pub length: usize,
    pub size: usize,
    pub offset: i64,
    pub str: *mut u8,
}

/// The leading fields of `swoole::Protocol` (`split_by_eof` is a C++ `bool`).
#[repr(C)]
pub struct Protocol {
    pub split_by_eof: u8,
    pub package_eof: [u8; 8],
    pub package_eof_len: u8,
    pub package_length_type: u8,
    pub package_length_size: u8,
    pub package_length_offset: u16,
    pub package_body_offset: u16,
    pub package_max_length: u32,
}

/// Where the length loop goes next.
enum Step {
    Recv,
    GetLength,
    Dispatch,
    Fail,
    Wait,
}

struct Recv<'a, E> {
    flags: u32,
    protocol: *const Protocol,
    buffer: *mut Buffer,
    effects: &'a mut E,
}

impl<E: Effects> Recv<'_, E> {
    fn op(&mut self, op: u32, arg: u64) -> i64 {
        self.effects.effect(op, arg, &mut self.flags)
    }
    fn flag(&self, bit: u32) -> bool {
        self.flags & bit != 0
    }
    /// `socket->recv_wait = wait`, applied by the caller before its next effect.
    fn set_wait(&mut self, wait: bool) {
        if wait {
            self.flags |= FLAG_WAIT;
        } else {
            self.flags &= !FLAG_WAIT;
        }
    }
    fn length(&self) -> usize {
        // SAFETY: `buffer` is the live String for this receive; effects change it only inside `op`.
        unsafe { (*self.buffer).length }
    }
    fn size(&self) -> usize {
        // SAFETY: as in `length`.
        unsafe { (*self.buffer).size }
    }
    fn offset(&self) -> i64 {
        // SAFETY: as in `length`.
        unsafe { (*self.buffer).offset }
    }
    fn set_length(&self, length: usize) {
        // SAFETY: as in `length`, and no other reference to the String is live.
        unsafe { (*self.buffer).length = length };
    }
    fn set_offset(&self, offset: i64) {
        // SAFETY: as in `set_length`.
        unsafe { (*self.buffer).offset = offset };
    }
    /// `buffer->clear()`.
    fn clear(&self) {
        self.set_length(0);
        self.set_offset(0);
    }
    fn eof_len(&self) -> usize {
        // SAFETY: `protocol` is live for this receive; callbacks may change it only inside `op`.
        usize::from(unsafe { (*self.protocol).package_eof_len })
    }
    fn max(&self) -> u32 {
        // SAFETY: as in `eof_len`.
        unsafe { (*self.protocol).package_max_length }
    }
    /// Appends `n` received bytes; `false` when they overrun the allocation.
    fn grow(&self, n: i64) -> bool {
        let (length, size) = (self.length(), self.size());
        match usize::try_from(n) {
            Ok(n) if n <= size - length => {
                self.set_length(length + n);
                true
            }
            _ => false,
        }
    }
    /// The buffer ends with the protocol's EOF (`length >= eof_len`).
    fn ends_with_eof(&self) -> bool {
        let (length, eof_len) = (self.length(), self.eof_len());
        // SAFETY: as in `length`; `str` holds `length` initialized bytes.
        let data = unsafe { (*self.buffer).str };
        // SAFETY: `eof_len <= length`, so the tail starts inside the buffer.
        let tail = unsafe { data.add(length - eof_len) };
        // SAFETY: the `eof_len` bytes from `tail` end at `length`.
        let tail = unsafe { super::bytes(tail, eof_len) };
        // SAFETY: as in `eof_len`; `package_eof` holds `package_eof_len` bytes.
        let eof_at = unsafe { addr_of!((*self.protocol).package_eof) };
        // SAFETY: as above.
        let eof = unsafe { super::bytes(eof_at.cast::<u8>(), eof_len) };
        matches!((tail, eof), (Some(tail), Some(eof)) if tail == eof)
    }

    fn length_protocol(&mut self, head: u32) -> i32 {
        if self.flag(FLAG_DYNAMIC) && head == 0 {
            return SW_ERR;
        }
        // A length header the buffer can never hold fails before any read.
        if self.header(head) > self.size() {
            return SW_ERR;
        }
        // Whether the last `recv` left space in the buffer, so the kernel had no more.
        let mut drained = false;
        let mut at = if self.flag(FLAG_SKIP) {
            self.op(OP_CLEAR_SKIP, 0);
            Step::GetLength
        } else {
            Step::Recv
        };
        loop {
            at = match at {
                Step::Recv => {
                    if self.flag(FLAG_REMOVED) {
                        return SW_OK;
                    }
                    let free = self.size() - self.length();
                    if free == 0 {
                        return SW_ERR;
                    }
                    match self.op(OP_RECV, free as u64) {
                        RECV_DONE => return SW_OK,
                        RECV_CLOSE | 0 => return SW_ERR,
                        n if n < 0 => return SW_ERR,
                        n if !self.grow(n) => return SW_ERR,
                        n => drained = n.unsigned_abs() < free as u64,
                    }
                    if !self.flag(FLAG_WAIT) {
                        Step::GetLength
                    } else if self.complete() {
                        Step::Dispatch
                    } else if self.flag(FLAG_TLS) {
                        Step::Recv
                    } else {
                        return SW_OK;
                    }
                }
                Step::GetLength => self.get_length(head, drained),
                Step::Dispatch => {
                    let offset = self.offset();
                    if self.op(OP_DISPATCH, offset.unsigned_abs()) < 0 {
                        return SW_ERR;
                    }
                    // A closed socket stops here, as upstream's does. A paused one
                    // still delivers the whole packages already read: only a partial
                    // one can be left, and the rest of it is still in the kernel.
                    if self.flag(FLAG_REMOVED) && !self.flag(FLAG_PAUSED) {
                        return SW_OK;
                    }
                    self.set_wait(false);
                    let offset = self.offset();
                    if usize::try_from(offset).is_ok_and(|offset| self.length() > offset) {
                        self.op(OP_REDUCE, offset.unsigned_abs());
                        Step::GetLength
                    } else {
                        self.clear();
                        if !self.flag(FLAG_TLS) || self.flag(FLAG_REMOVED) {
                            return SW_OK;
                        }
                        Step::Recv
                    }
                }
                Step::Fail => return SW_ERR,
                Step::Wait => return SW_OK,
            };
        }
    }

    /// Bytes up to and including the length field: `package_length_offset` plus `head`.
    fn header(&self, head: u32) -> usize {
        // SAFETY: as in `eof_len`.
        (u32::from(unsafe { (*self.protocol).package_length_offset }) + head) as usize
    }

    /// The next package's length from the buffered bytes: dispatch it if it is all
    /// here, else read more (unless the socket left the reactor).
    fn get_length(&mut self, head: u32, drained: bool) -> Step {
        let packet = self.op(OP_GET_LENGTH, 0);
        let header = self.header(head);
        if packet < 0 {
            self.op(OP_LOG_MALFORMED, 0);
            return self.fail(header);
        }
        if packet == 0 {
            // The whole length header is here and holds no length.
            if self.length() >= header {
                self.op(OP_LOG_NO_LENGTH, 0);
                return self.fail(header);
            }
            return self.more(drained);
        }
        if packet > i64::from(self.max()) {
            self.op(OP_LOG_TOO_LARGE, packet.unsigned_abs());
            return self.fail(header);
        }
        if usize::try_from(packet).map_or(true, |packet| self.size() < packet) {
            self.op(OP_EXTEND, packet.unsigned_abs());
        }
        self.set_wait(true);
        self.set_offset(packet);
        if self.complete() {
            Step::Dispatch
        } else {
            self.more(drained)
        }
    }

    /// A bad package closes the connection; with bytes past its `header` buffered,
    /// the close resets, as upstream's does over the bytes it never read.
    fn fail(&mut self, header: usize) -> Step {
        if self.length() > header {
            self.op(OP_RESET, 0);
        }
        Step::Fail
    }

    /// Read more for a partial package, unless the socket left the reactor (a pause
    /// mid-buffer) or the last read drained it (TLS may still hold decrypted bytes).
    fn more(&self, drained: bool) -> Step {
        if self.flag(FLAG_REMOVED) || (drained && !self.flag(FLAG_TLS)) {
            Step::Wait
        } else {
            Step::Recv
        }
    }

    /// The buffer holds the whole awaited package (`length >= offset`).
    fn complete(&self) -> bool {
        usize::try_from(self.offset()).is_ok_and(|offset| self.length() >= offset)
    }

    fn eof_protocol(&mut self) -> i32 {
        let mut again = false;
        loop {
            let size = self.size() - self.length();
            match self.op(OP_RECV, size.min(BUFFER_SIZE_STD) as u64) {
                RECV_DONE => return SW_OK,
                RECV_CLOSE | 0 => return SW_ERR,
                n if n < 0 => return SW_ERR,
                n if !self.grow(n) => return SW_ERR,
                _ => {}
            }
            if self.length() < self.eof_len() {
                return SW_OK;
            }
            // SAFETY: as in `eof_len`.
            if unsafe { (*self.protocol).split_by_eof } != 0 {
                match self.split_by_eof() {
                    SW_CONTINUE => again = true,
                    SW_CLOSE => return SW_ERR,
                    _ => return SW_OK,
                }
            } else if self.ends_with_eof() {
                let length = self.length();
                self.set_offset(length.cast_signed() as i64);
                if self.op(OP_DISPATCH, length as u64) < 0 {
                    return SW_ERR;
                }
                if self.flag(FLAG_REMOVED) {
                    return SW_OK;
                }
                self.clear();
                if self.flag(FLAG_TLS) && self.op(OP_SSL_PENDING, 0) > 0 {
                    continue;
                }
                return SW_OK;
            }
            let (length, size, max) = (self.length(), self.size(), self.max());
            if length == max as usize {
                self.op(OP_LOG_TOO_LARGE, length as u64);
                return SW_ERR;
            }
            if length == size {
                again = true;
                if size < max as usize {
                    let page = self.op(OP_PAGE, 0);
                    let Some(page) = usize::try_from(page).ok().filter(|&page| page > 0) else {
                        return SW_ERR;
                    };
                    let double = size.wrapping_mul(2);
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "upstream's uint32_t extend_size"
                    )]
                    let extend = double.wrapping_add(page - double % page) as u32;
                    self.op(OP_EXTEND, u64::from(extend.min(max)));
                }
            }
            if !again {
                return SW_OK;
            }
        }
    }

    fn split_by_eof(&mut self) -> i32 {
        if self.length() < self.eof_len() {
            return SW_CONTINUE;
        }
        let n = self.op(OP_SPLIT, 0);
        if self.flag(FLAG_REMOVED) || n < 0 {
            return SW_CLOSE;
        }
        if n == 0 {
            return SW_CONTINUE;
        }
        if usize::try_from(n).is_ok_and(|n| n < self.length()) {
            self.op(OP_REDUCE, n.unsigned_abs());
            let offset = self.length().saturating_sub(self.eof_len());
            self.set_offset(offset.cast_signed() as i64);
        } else {
            self.clear();
        }
        i32::from(self.flag(FLAG_TLS))
    }
}

/// Runs one receive of `mode` and returns its `SW_*` result. `head` is the
/// package length size (`get_package_length_size` or the fixed size); `flags`
/// holds the socket's flags at entry and its `recv_wait` on return.
///
/// # Safety
/// `protocol` and `buffer` are live for the receive, and `effects` changes them
/// only inside [`Effects::effect`].
pub unsafe fn recv<E: Effects>(
    protocol: *const Protocol,
    buffer: *mut Buffer,
    mode: u32,
    head: u32,
    flags: &mut u32,
    effects: &mut E,
) -> i32 {
    let mut recv = Recv {
        flags: *flags,
        protocol,
        buffer,
        effects,
    };
    if recv.length() > recv.size() {
        return SW_ERR;
    }
    let result = match mode {
        MODE_LENGTH => recv.length_protocol(head),
        MODE_EOF => recv.eof_protocol(),
        MODE_SPLIT => recv.split_by_eof(),
        _ => SW_ERR,
    };
    *flags = recv.flags;
    result
}

/// [`recv`]'s C form, for the oracle's native candidate: `io` holds the
/// arguments and `call(context, op, arg)` performs each effect and refreshes
/// `io.flags`.
#[repr(C)]
pub struct SwrFramingIo {
    pub protocol: *const Protocol,
    pub buffer: *mut Buffer,
    pub mode: u32,
    pub head: u32,
    pub flags: u32,
}

/// One native effect on the socket, buffer or protocol in `context`.
pub type FramingCall = unsafe extern "C" fn(context: *mut c_void, op: u32, arg: u64) -> i64;

/// Any op: a native exception was caught; native code rethrows it after return.
pub const THROWN: i64 = i64::MIN;

struct NativeEffects {
    io: *mut SwrFramingIo,
    context: *mut c_void,
    call: FramingCall,
    thrown: bool,
}

impl Effects for NativeEffects {
    /// After a `THROWN` op, no further effect runs: each op fails and the socket
    /// reads as removed, so the loop returns within a few steps.
    fn effect(&mut self, op: u32, arg: u64, flags: &mut u32) -> i64 {
        // The native effect tables (the oracle's patched base.cc) predate the reset,
        // which only changes how the connection closes.
        if op == OP_RESET {
            return 0;
        }
        if !self.thrown {
            // SAFETY: `io` is the caller's live receive; native code reads it only inside `call`.
            unsafe { (*self.io).flags = *flags };
            // SAFETY: `call` and `context` are the native pair lent for this receive.
            let result = unsafe { (self.call)(self.context, op, arg) };
            // SAFETY: as above; `call` has returned.
            *flags = unsafe { (*self.io).flags };
            self.thrown = result == THROWN;
            if !self.thrown {
                return result;
            }
        }
        *flags |= FLAG_REMOVED;
        RECV_CLOSE
    }
}

/// Runs one receive of `io.mode` and returns its `SW_*` result. After a `THROWN`
/// op it runs no further effect; native code rethrows.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_framing_recv(
    io: *mut SwrFramingIo,
    context: *mut c_void,
    call: Option<FramingCall>,
) -> i32 {
    // SAFETY: the caller passes null or its live receive.
    let (Some(shared), Some(call)) = (unsafe { io.as_ref() }, call) else {
        return SW_ERR;
    };
    if shared.protocol.is_null() || shared.buffer.is_null() {
        return SW_ERR;
    }
    let (protocol, buffer, mode, head, mut flags) = (
        shared.protocol,
        shared.buffer,
        shared.mode,
        shared.head,
        shared.flags,
    );
    let mut effects = NativeEffects {
        io,
        context,
        call,
        thrown: false,
    };
    // SAFETY: the caller's live protocol and buffer, changed only inside `call`.
    let status = unsafe { recv(protocol, buffer, mode, head, &mut flags, &mut effects) };
    // SAFETY: the caller's live receive.
    unsafe { (*io).flags = flags };
    if effects.thrown { SW_ERR } else { status }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ptr;

    /// A fake native side: scripted reads, a growable buffer and a log of effects.
    struct Native {
        mode: u32,
        head: u32,
        flags: u32,
        protocol: Protocol,
        buffer: Buffer,
        storage: Vec<u8>,
        chunks: Vec<&'static [u8]>,
        /// `FLAG_REMOVED` and `FLAG_PAUSED`, as the socket reports them.
        gone: u32,
        /// What `gone` becomes at the first dispatch (a callback closing or pausing).
        gone_on_dispatch: u32,
        wait: bool,
        events: Vec<String>,
    }

    impl Native {
        fn new(mode: u32, size: usize, chunks: Vec<&'static [u8]>) -> Box<Self> {
            let mut native = Box::new(Native {
                mode,
                head: 1,
                flags: 0,
                protocol: Protocol {
                    split_by_eof: 0,
                    package_eof: *b"\r\n\0\0\0\0\0\0",
                    package_eof_len: 2,
                    package_length_type: b'C',
                    package_length_size: 1,
                    package_length_offset: 0,
                    package_body_offset: 1,
                    package_max_length: 64,
                },
                buffer: Buffer {
                    length: 0,
                    size,
                    offset: 0,
                    str: ptr::null_mut(),
                },
                storage: vec![0; size],
                chunks,
                gone: 0,
                gone_on_dispatch: 0,
                wait: false,
                events: Vec::new(),
            });
            native.buffer.str = native.storage.as_mut_ptr();
            native
        }
        fn data(&self) -> &[u8] {
            &self.storage[..self.buffer.length]
        }
        fn apply(&mut self, op: u32, arg: u64) -> i64 {
            let arg = usize::try_from(arg).unwrap();
            let length = self.buffer.length;
            match op {
                OP_RECV => {
                    self.events.push(format!("recv:{arg}"));
                    if self.chunks.is_empty() {
                        return RECV_DONE;
                    }
                    let chunk = self.chunks[0];
                    let n = chunk.len().min(arg);
                    self.storage[length..length + n].copy_from_slice(&chunk[..n]);
                    if n == chunk.len() {
                        self.chunks.remove(0);
                    } else {
                        self.chunks[0] = &chunk[n..];
                    }
                    i64::try_from(n).unwrap()
                }
                OP_GET_LENGTH => {
                    self.events.push(format!("length:{length}"));
                    self.data().first().map_or(0, |&n| i64::from(n) + 1)
                }
                OP_EXTEND => {
                    self.events.push(format!("extend:{arg}"));
                    self.storage.resize(arg, 0);
                    self.buffer.size = arg;
                    self.buffer.str = self.storage.as_mut_ptr();
                    0
                }
                OP_DISPATCH => {
                    let packet = String::from_utf8_lossy(&self.storage[..arg]).into_owned();
                    self.events.push(format!("package:{packet}"));
                    self.gone |= self.gone_on_dispatch;
                    0
                }
                OP_REDUCE => {
                    self.events.push(format!("reduce:{arg}"));
                    self.storage.copy_within(arg..length, 0);
                    self.buffer.length -= arg;
                    self.buffer.offset = 0;
                    0
                }
                OP_SPLIT => {
                    let mut end = 0;
                    while let Some(at) = self.storage[end..length]
                        .windows(2)
                        .position(|w| w == b"\r\n")
                    {
                        let packet = String::from_utf8_lossy(&self.storage[end..end + at + 2]);
                        self.events.push(format!("package:{packet}"));
                        end += at + 2;
                    }
                    i64::try_from(end).unwrap()
                }
                OP_PAGE => 4096,
                OP_CLEAR_SKIP => 0,
                _ => {
                    self.events.push(format!("op{op}:{arg}"));
                    0
                }
            }
        }
    }

    /// The fake by raw pointer, as the native side holds it: `recv` reads the
    /// protocol and buffer through pointers into it between effects.
    impl Effects for *mut Native {
        fn effect(&mut self, op: u32, arg: u64, flags: &mut u32) -> i64 {
            // SAFETY: `run`'s live fake; `recv` holds no reference into it.
            let native = unsafe { &mut **self };
            native.wait = *flags & FLAG_WAIT != 0;
            let result = native.apply(op, arg);
            *flags = native.gone | if native.wait { FLAG_WAIT } else { 0 };
            result
        }
    }

    fn run(native: &mut Native) -> i32 {
        let mut flags = native.flags;
        let mut native = ptr::from_mut(native);
        // SAFETY: the fake's protocol and buffer, changed only inside its effects.
        let status = unsafe {
            recv(
                &raw const (*native).protocol,
                &raw mut (*native).buffer,
                (*native).mode,
                (*native).head,
                &mut flags,
                &mut native,
            )
        };
        // SAFETY: the receive has returned.
        let native = unsafe { &mut *native };
        native.flags = flags;
        native.wait = flags & FLAG_WAIT != 0;
        status
    }

    #[test]
    fn length_protocol_reads_what_is_there_then_waits() {
        // Each chunk is one arrival: a short read means the kernel had no more.
        let mut native = Native::new(MODE_LENGTH, 2, vec![b"\x03", b"abc\x01"]);
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(native.events, ["recv:2", "length:1", "extend:4"]);
        assert!(native.wait, "waits for the body");
        native.events.clear();
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(native.events, ["recv:3", "package:\x03abc"]);
        assert_eq!(
            (native.buffer.length, native.buffer.offset),
            (0, 0),
            "cleared"
        );
        // The next call finds the next header and waits for its body.
        native.events.clear();
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(native.events, ["recv:4", "length:1"]);
        assert!(native.wait, "waits for the next body");
        assert_eq!(native.buffer.offset, 2);
    }

    #[test]
    fn length_protocol_dispatches_every_package_one_read_brings() {
        let mut native = Native::new(MODE_LENGTH, 16, vec![b"\x01a\x02bc\x03d"]);
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(
            native.events,
            [
                "recv:16",
                "length:7",
                "package:\x01a",
                "reduce:2",
                "length:5",
                "package:\x02bc",
                "reduce:3",
                "length:2"
            ]
        );
        assert_eq!(native.data(), b"\x03d", "the partial package stays");
        assert!(native.wait, "and waits for its last two bytes");
    }

    #[test]
    fn length_protocol_closed_mid_buffer_stops_and_paused_delivers_the_rest() {
        let mut native = Native::new(MODE_LENGTH, 16, vec![b"\x01a\x01b"]);
        native.gone_on_dispatch = FLAG_REMOVED;
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(native.events, ["recv:16", "length:4", "package:\x01a"]);

        let mut native = Native::new(MODE_LENGTH, 16, vec![b"\x01a\x01b\x02", b"c"]);
        native.gone_on_dispatch = FLAG_REMOVED | FLAG_PAUSED;
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(
            native.events,
            [
                "recv:16",
                "length:5",
                "package:\x01a",
                "reduce:2",
                "length:3",
                "package:\x01b",
                "reduce:2",
                "length:1"
            ],
            "no read while paused"
        );
        assert!(native.wait, "the partial package waits for resume");
    }

    #[test]
    fn length_protocol_dispatches_back_to_back_packages() {
        let mut native = Native::new(MODE_LENGTH, 8, vec![b"\x10"]);
        native.storage[..3].copy_from_slice(b"\x01a\x00");
        native.buffer.length = 3;
        native.flags = FLAG_SKIP;
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(
            native.events,
            [
                "length:3",
                "package:\x01a",
                "reduce:2",
                "length:1",
                "package:\0"
            ]
        );
        assert_eq!(
            (native.buffer.length, native.buffer.offset),
            (0, 0),
            "cleared"
        );
        native.events.clear();
        native.protocol.package_max_length = 4;
        assert_eq!(run(&mut native), SW_ERR);
        assert_eq!(native.events, ["recv:8", "length:1", "op13:17"]);
    }

    #[test]
    fn a_bad_package_with_bytes_past_its_header_closes_with_a_reset() {
        let mut native = Native::new(MODE_LENGTH, 8, vec![b"\x10abc"]);
        native.protocol.package_max_length = 4;
        assert_eq!(run(&mut native), SW_ERR);
        assert_eq!(native.events, ["recv:8", "length:4", "op13:17", "op14:0"]);
    }

    #[test]
    fn length_protocol_stops_on_removed_and_dynamic_zero() {
        let mut native = Native::new(MODE_LENGTH, 8, vec![b"\x01"]);
        native.flags = FLAG_REMOVED;
        native.gone = FLAG_REMOVED;
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(native.events, Vec::<String>::new());
        native.flags = FLAG_DYNAMIC;
        native.head = 0;
        assert_eq!(run(&mut native), SW_ERR);
        assert_eq!(native.events, Vec::<String>::new());
    }

    #[test]
    fn eof_protocol_grows_then_dispatches() {
        let mut native = Native::new(MODE_EOF, 4, vec![b"abcd", b"e\r\n"]);
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(
            native.events,
            ["recv:4", "extend:64", "recv:60", "package:abcde\r\n"]
        );
        assert_eq!(
            (native.buffer.length, native.buffer.offset),
            (0, 0),
            "cleared"
        );
    }

    #[test]
    fn split_keeps_the_residual() {
        let mut native = Native::new(MODE_EOF, 16, vec![b"a\r\nb\r\nc"]);
        native.protocol.split_by_eof = 1;
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(
            native.events,
            ["recv:16", "package:a\r\n", "package:b\r\n", "reduce:6"]
        );
        assert_eq!(native.data(), b"c");
        assert_eq!(native.buffer.offset, 0);
    }

    #[test]
    fn rejects_overrun() {
        let mut native = Native::new(MODE_EOF, 2, vec![]);
        native.buffer.length = 3;
        assert_eq!(run(&mut native), SW_ERR);
        assert_eq!(native.events, Vec::<String>::new());
    }

    /// The C form over the same fake, throwing at `OP_EXTEND`.
    unsafe extern "C" fn throwing(context: *mut c_void, op: u32, arg: u64) -> i64 {
        // SAFETY: the test passes its live `Native` as the context.
        let native = unsafe { &mut *context.cast::<Native>() };
        let result = native.apply(op, arg);
        if op == OP_EXTEND { THROWN } else { result }
    }

    #[test]
    fn a_thrown_effect_runs_no_further_effect() {
        let mut native = Native::new(MODE_LENGTH, 2, vec![b"\x05"]);
        // One pointer for the context and the fields `io` aliases, so the
        // effect's `&mut Native` doesn't invalidate `io`'s pointers.
        let fake = ptr::from_mut(&mut *native);
        let mut io = SwrFramingIo {
            // SAFETY: `fake` is the live boxed fake.
            protocol: unsafe { &raw const (*fake).protocol },
            // SAFETY: as above.
            buffer: unsafe { &raw mut (*fake).buffer },
            mode: MODE_LENGTH,
            head: 1,
            flags: 0,
        };
        let context = fake.cast();
        // SAFETY: `io` points into the fake passed as the context.
        let status = unsafe { swr_v1_framing_recv(&raw mut io, context, Some(throwing)) };
        assert_eq!(status, SW_ERR);
        assert_eq!(native.events, ["recv:2", "length:1", "extend:6"]);
        // SAFETY: null is accepted.
        let status = unsafe { swr_v1_framing_recv(ptr::null_mut(), context, Some(throwing)) };
        assert_eq!(status, SW_ERR);
    }
}
