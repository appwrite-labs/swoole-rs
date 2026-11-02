//! `Protocol::recv_with_length_protocol`, `recv_with_eof_protocol` and
//! `recv_split_by_eof`, each one call. The receive buffer (`swoole::String`) and
//! the protocol's leading fields are read and written in place (layouts pinned by
//! native `static_assert`s); native code performs each socket, callback, allocation
//! or log effect through one `call(context, op, arg)` and refreshes
//! [`SwrFramingIo::flags`] after it.
use core::ffi::c_void;
use core::ptr::addr_of;

/// One native effect on the socket, buffer or protocol in `context`.
pub(crate) type FramingCall = unsafe extern "C" fn(context: *mut c_void, op: u32, arg: u64) -> i64;

pub(crate) const MODE_LENGTH: u32 = 0;
pub(crate) const MODE_EOF: u32 = 1;
pub(crate) const MODE_SPLIT: u32 = 2;

/// `socket->recv` of arg bytes into the buffer's tail, with upstream's read-error
/// handling and warning: the byte count, or `RECV_CLOSE`/`RECV_DONE`.
pub(crate) const OP_RECV: u32 = 1;
/// `get_package_length` over the whole buffer.
pub(crate) const OP_GET_LENGTH: u32 = 2;
pub(crate) const OP_EXTEND: u32 = 3;
/// `socket->recv_wait = arg`.
pub(crate) const OP_WAIT: u32 = 4;
/// `onPackage` of arg bytes from the buffer start.
pub(crate) const OP_DISPATCH: u32 = 5;
pub(crate) const OP_REDUCE: u32 = 6;
pub(crate) const OP_CLEAR: u32 = 7;
/// `buffer->split` on the EOF, each part to `onPackage`.
pub(crate) const OP_SPLIT: u32 = 8;
pub(crate) const OP_SSL_PENDING: u32 = 9;
/// `socket->skip_recv = 0`.
pub(crate) const OP_CLEAR_SKIP: u32 = 10;
/// `swoole_pagesize()`.
pub(crate) const OP_PAGE: u32 = 11;
pub(crate) const OP_LOG_MALFORMED: u32 = 12;
pub(crate) const OP_LOG_NO_LENGTH: u32 = 13;
/// arg: the packet length (length protocol).
pub(crate) const OP_LOG_TOO_LARGE: u32 = 14;

/// `OP_RECV`: the connection closes (`SW_ERR`).
pub(crate) const RECV_CLOSE: i64 = -1;
/// `OP_RECV`: the read ends with `SW_OK` (would block, logged error, removed socket).
pub(crate) const RECV_DONE: i64 = -2;
/// Any op: a native exception was caught; native code rethrows it after return.
pub(crate) const THROWN: i64 = i64::MIN;

/// [`SwrFramingIo::flags`] bits: `socket->removed`, `recv_wait`, `ssl != nullptr`,
/// and, at entry only, `skip_recv` and a dynamic `get_package_length_size`.
pub(crate) const FLAG_REMOVED: u32 = 1;
pub(crate) const FLAG_WAIT: u32 = 2;
pub(crate) const FLAG_TLS: u32 = 4;
pub(crate) const FLAG_SKIP: u32 = 8;
pub(crate) const FLAG_DYNAMIC: u32 = 16;

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

#[repr(C)]
pub struct SwrFramingIo {
    pub protocol: *const Protocol,
    pub buffer: *mut Buffer,
    pub mode: u32,
    /// The package length size (`get_package_length_size` or the fixed size).
    pub head: u32,
    pub flags: u32,
}

/// A native exception is pending.
struct Thrown;

struct Recv {
    io: *const SwrFramingIo,
    protocol: *const Protocol,
    buffer: *mut Buffer,
    context: *mut c_void,
    call: FramingCall,
}

impl Recv {
    fn op(&self, op: u32, arg: u64) -> Result<i64, Thrown> {
        // SAFETY: `call` and `context` are the native pair lent for this receive.
        let result = unsafe { (self.call)(self.context, op, arg) };
        if result == THROWN {
            Err(Thrown)
        } else {
            Ok(result)
        }
    }
    fn flag(&self, bit: u32) -> bool {
        // SAFETY: `io` is live for this receive; native code writes it only inside `op`.
        let flags = unsafe { (*self.io).flags };
        flags & bit != 0
    }
    fn length(&self) -> usize {
        // SAFETY: `buffer` is the live String for this receive; native code changes it only inside `op`.
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

    fn length_protocol(&self, head: u32) -> Result<i32, Thrown> {
        enum At {
            Recv,
            GetLength,
            Dispatch,
        }
        if self.flag(FLAG_DYNAMIC) && head == 0 {
            return Ok(SW_ERR);
        }
        let mut recv_size: u32 = 0;
        let mut at = if self.flag(FLAG_SKIP) {
            self.op(OP_CLEAR_SKIP, 0)?;
            At::GetLength
        } else {
            At::Recv
        };
        loop {
            at = match at {
                At::Recv => {
                    if self.flag(FLAG_REMOVED) {
                        return Ok(SW_OK);
                    }
                    let (length, size, offset) = (self.length(), self.size(), self.offset());
                    recv_size = if offset > 0 {
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "upstream's uint32_t recv_size"
                        )]
                        let n = offset.unsigned_abs().wrapping_sub(length as u64) as u32;
                        n
                    } else {
                        // SAFETY: as in `eof_len`.
                        u32::from(unsafe { (*self.protocol).package_length_offset }) + head
                    };
                    if recv_size as usize > size - length {
                        return Ok(SW_ERR);
                    }
                    match self.op(OP_RECV, u64::from(recv_size))? {
                        RECV_DONE => return Ok(SW_OK),
                        RECV_CLOSE | 0 => return Ok(SW_ERR),
                        n if n < 0 => return Ok(SW_ERR),
                        n if !self.grow(n) => return Ok(SW_ERR),
                        _ => {}
                    }
                    if !self.flag(FLAG_WAIT) {
                        At::GetLength
                    } else if self.complete() {
                        At::Dispatch
                    } else if self.flag(FLAG_TLS) {
                        At::Recv
                    } else {
                        return Ok(SW_OK);
                    }
                }
                At::GetLength => {
                    let packet = self.op(OP_GET_LENGTH, 0)?;
                    if packet < 0 {
                        self.op(OP_LOG_MALFORMED, 0)?;
                        return Ok(SW_ERR);
                    }
                    if packet == 0 {
                        if self.length() == recv_size as usize {
                            self.op(OP_LOG_NO_LENGTH, 0)?;
                            return Ok(SW_ERR);
                        }
                        return Ok(SW_OK);
                    }
                    if packet > i64::from(self.max()) {
                        self.op(OP_LOG_TOO_LARGE, packet.unsigned_abs())?;
                        return Ok(SW_ERR);
                    }
                    if usize::try_from(packet).map_or(true, |packet| self.size() < packet) {
                        self.op(OP_EXTEND, packet.unsigned_abs())?;
                    }
                    self.op(OP_WAIT, 1)?;
                    self.set_offset(packet);
                    if self.complete() {
                        At::Dispatch
                    } else {
                        At::Recv
                    }
                }
                At::Dispatch => {
                    let offset = self.offset();
                    if self.op(OP_DISPATCH, offset.unsigned_abs())? < 0 {
                        return Ok(SW_ERR);
                    }
                    if self.flag(FLAG_REMOVED) {
                        return Ok(SW_OK);
                    }
                    self.op(OP_WAIT, 0)?;
                    let offset = self.offset();
                    if usize::try_from(offset).is_ok_and(|offset| self.length() > offset) {
                        self.op(OP_REDUCE, offset.unsigned_abs())?;
                        At::GetLength
                    } else {
                        self.op(OP_CLEAR, 0)?;
                        if !self.flag(FLAG_TLS) {
                            return Ok(SW_OK);
                        }
                        At::Recv
                    }
                }
            };
        }
    }

    /// The buffer holds the whole awaited package (`length >= offset`).
    fn complete(&self) -> bool {
        usize::try_from(self.offset()).is_ok_and(|offset| self.length() >= offset)
    }

    fn eof_protocol(&self) -> Result<i32, Thrown> {
        let mut again = false;
        loop {
            let size = self.size() - self.length();
            match self.op(OP_RECV, size.min(BUFFER_SIZE_STD) as u64)? {
                RECV_DONE => return Ok(SW_OK),
                RECV_CLOSE | 0 => return Ok(SW_ERR),
                n if n < 0 => return Ok(SW_ERR),
                n if !self.grow(n) => return Ok(SW_ERR),
                _ => {}
            }
            if self.length() < self.eof_len() {
                return Ok(SW_OK);
            }
            // SAFETY: as in `eof_len`.
            if unsafe { (*self.protocol).split_by_eof } != 0 {
                match self.split_by_eof()? {
                    SW_CONTINUE => again = true,
                    SW_CLOSE => return Ok(SW_ERR),
                    _ => return Ok(SW_OK),
                }
            } else if self.ends_with_eof() {
                let length = self.length();
                self.set_offset(length.cast_signed() as i64);
                if self.op(OP_DISPATCH, length as u64)? < 0 {
                    return Ok(SW_ERR);
                }
                if self.flag(FLAG_REMOVED) {
                    return Ok(SW_OK);
                }
                self.op(OP_CLEAR, 0)?;
                if self.flag(FLAG_TLS) && self.op(OP_SSL_PENDING, 0)? > 0 {
                    continue;
                }
                return Ok(SW_OK);
            }
            let (length, size, max) = (self.length(), self.size(), self.max());
            if length == max as usize {
                self.op(OP_LOG_TOO_LARGE, length as u64)?;
                return Ok(SW_ERR);
            }
            if length == size {
                again = true;
                if size < max as usize {
                    let page = self.op(OP_PAGE, 0)?;
                    let Some(page) = usize::try_from(page).ok().filter(|&page| page > 0) else {
                        return Ok(SW_ERR);
                    };
                    let double = size.wrapping_mul(2);
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "upstream's uint32_t extend_size"
                    )]
                    let extend = double.wrapping_add(page - double % page) as u32;
                    self.op(OP_EXTEND, u64::from(extend.min(max)))?;
                }
            }
            if !again {
                return Ok(SW_OK);
            }
        }
    }

    fn split_by_eof(&self) -> Result<i32, Thrown> {
        if self.length() < self.eof_len() {
            return Ok(SW_CONTINUE);
        }
        let n = self.op(OP_SPLIT, 0)?;
        if self.flag(FLAG_REMOVED) || n < 0 {
            return Ok(SW_CLOSE);
        }
        if n == 0 {
            return Ok(SW_CONTINUE);
        }
        if usize::try_from(n).is_ok_and(|n| n < self.length()) {
            self.op(OP_REDUCE, n.unsigned_abs())?;
            let offset = self.length().saturating_sub(self.eof_len());
            self.set_offset(offset.cast_signed() as i64);
        } else {
            self.op(OP_CLEAR, 0)?;
        }
        Ok(i32::from(self.flag(FLAG_TLS)))
    }
}

/// Runs one receive of `io.mode` and returns its `SW_*` result. After a `THROWN`
/// op it returns at once; native code rethrows.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_framing_recv(
    io: *const SwrFramingIo,
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
    let (mode, head) = (shared.mode, shared.head);
    let recv = Recv {
        io,
        protocol: shared.protocol,
        buffer: shared.buffer,
        context,
        call,
    };
    if recv.length() > recv.size() {
        return SW_ERR;
    }
    let result = match mode {
        MODE_LENGTH => recv.length_protocol(head),
        MODE_EOF => recv.eof_protocol(),
        MODE_SPLIT => recv.split_by_eof(),
        _ => Ok(SW_ERR),
    };
    result.unwrap_or(SW_ERR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ptr;

    /// A fake native side: scripted reads, a growable buffer and a log of effects.
    struct Native {
        io: SwrFramingIo,
        protocol: Protocol,
        buffer: Buffer,
        storage: Vec<u8>,
        chunks: Vec<&'static [u8]>,
        removed: bool,
        wait: bool,
        throw_on_extend: bool,
        events: Vec<String>,
    }

    impl Native {
        fn new(mode: u32, size: usize, chunks: Vec<&'static [u8]>) -> Box<Self> {
            let mut native = Box::new(Native {
                io: SwrFramingIo {
                    protocol: ptr::null(),
                    buffer: ptr::null_mut(),
                    mode,
                    head: 1,
                    flags: 0,
                },
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
                removed: false,
                wait: false,
                throw_on_extend: false,
                events: Vec::new(),
            });
            native.buffer.str = native.storage.as_mut_ptr();
            native.io.protocol = &raw const native.protocol;
            native.io.buffer = &raw mut native.buffer;
            native
        }
        fn data(&self) -> &[u8] {
            &self.storage[..self.buffer.length]
        }
        fn effect(&mut self, op: u32, arg: u64) -> i64 {
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
                    if self.throw_on_extend {
                        return THROWN;
                    }
                    self.storage.resize(arg, 0);
                    self.buffer.size = arg;
                    self.buffer.str = self.storage.as_mut_ptr();
                    0
                }
                OP_WAIT => {
                    self.wait = arg != 0;
                    0
                }
                OP_DISPATCH => {
                    let packet = String::from_utf8_lossy(&self.storage[..arg]).into_owned();
                    self.events.push(format!("package:{packet}"));
                    0
                }
                OP_REDUCE => {
                    self.events.push(format!("reduce:{arg}"));
                    self.storage.copy_within(arg..length, 0);
                    self.buffer.length -= arg;
                    self.buffer.offset = 0;
                    0
                }
                OP_CLEAR => {
                    self.events.push("clear".into());
                    self.buffer.length = 0;
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

    unsafe extern "C" fn call(context: *mut c_void, op: u32, arg: u64) -> i64 {
        // SAFETY: tests pass their live `Native` as the context.
        let native = unsafe { &mut *context.cast::<Native>() };
        let result = native.effect(op, arg);
        native.io.flags = u32::from(native.removed) | if native.wait { FLAG_WAIT } else { 0 };
        result
    }

    fn run(native: &mut Native) -> i32 {
        let io = &raw const native.io;
        // SAFETY: `io` and the context are the same live fake.
        unsafe { swr_v1_framing_recv(io, ptr::from_mut(native).cast(), Some(call)) }
    }

    #[test]
    fn length_protocol_reads_header_then_body() {
        let mut native = Native::new(MODE_LENGTH, 2, vec![b"\x03", b"abc\x01"]);
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(
            native.events,
            [
                "recv:1",
                "length:1",
                "extend:4",
                "recv:3",
                "package:\x03abc",
                "clear"
            ]
        );
        // The next call finds the next header and waits for its body.
        native.events.clear();
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(native.events, ["recv:1", "length:1", "recv:1"]);
        assert!(native.wait);
        assert_eq!(native.buffer.offset, 2);
    }

    #[test]
    fn length_protocol_dispatches_back_to_back_packages() {
        let mut native = Native::new(MODE_LENGTH, 8, vec![b"\x10"]);
        native.storage[..3].copy_from_slice(b"\x01a\x00");
        native.buffer.length = 3;
        native.io.flags = FLAG_SKIP;
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(
            native.events,
            [
                "length:3",
                "package:\x01a",
                "reduce:2",
                "length:1",
                "package:\0",
                "clear"
            ]
        );
        native.events.clear();
        native.protocol.package_max_length = 4;
        assert_eq!(run(&mut native), SW_ERR);
        assert_eq!(native.events, ["recv:1", "length:1", "op14:17"]);
    }

    #[test]
    fn length_protocol_stops_on_removed_and_dynamic_zero() {
        let mut native = Native::new(MODE_LENGTH, 8, vec![b"\x01"]);
        native.io.flags = FLAG_REMOVED;
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(native.events, Vec::<String>::new());
        native.io.flags = FLAG_DYNAMIC;
        native.io.head = 0;
        assert_eq!(run(&mut native), SW_ERR);
        assert_eq!(native.events, Vec::<String>::new());
    }

    #[test]
    fn eof_protocol_grows_then_dispatches() {
        let mut native = Native::new(MODE_EOF, 4, vec![b"abcd", b"e\r\n"]);
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(
            native.events,
            [
                "recv:4",
                "extend:64",
                "recv:60",
                "package:abcde\r\n",
                "clear"
            ]
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
    fn a_thrown_effect_returns_at_once() {
        let mut native = Native::new(MODE_LENGTH, 2, vec![b"\x05"]);
        native.throw_on_extend = true;
        assert_eq!(run(&mut native), SW_ERR);
        assert_eq!(native.events, ["recv:1", "length:1", "extend:6"]);
        assert!(!native.wait);
    }

    #[test]
    fn rejects_null_and_overrun() {
        // SAFETY: null is accepted.
        let result = unsafe { swr_v1_framing_recv(ptr::null(), ptr::null_mut(), Some(call)) };
        assert_eq!(result, SW_ERR);
        let mut native = Native::new(MODE_EOF, 2, vec![]);
        native.buffer.length = 3;
        assert_eq!(run(&mut native), SW_ERR);
        assert_eq!(native.events, Vec::<String>::new());
    }
}
