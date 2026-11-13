//! `Protocol::recv_with_length_protocol`, `recv_with_eof_protocol` and
//! `recv_split_by_eof`, each one call. The length loop reads greedily: one `recv` of
//! the buffer's free space, then every whole package in it, where upstream reads the
//! length header and then exactly the rest of the package, a `recv` each. The caller
//! holds the receive buffer (`swoole::String`) and the protocol: the loop reads and
//! sets them through [`Effects`], which performs each socket, callback, allocation or
//! log effect in [`Effects::effect`].

/// The socket, buffer and protocol of one receive. The loop holds nothing across
/// an effect: it reads the buffer and protocol again after each one.
pub trait Effects {
    /// Performs `op`: applies [`FLAG_WAIT`] from `flags` to the socket first, and
    /// refreshes `flags` from the socket after.
    fn effect(&mut self, op: Op, arg: u64, flags: &mut u32) -> i64;
    /// The buffer's `length`, `size` and `offset` now.
    fn buffer(&self) -> Buffer;
    /// Sets the buffer's `length` and `offset`.
    fn set_buffer(&mut self, length: usize, offset: i64);
    /// The buffer's `length` received bytes.
    fn data(&self) -> &[u8];
    /// The protocol's settings now.
    fn protocol(&self) -> Protocol;
}

/// The protocol a receive frames by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `recv_with_length_protocol`.
    Length,
    /// `recv_with_eof_protocol`.
    Eof,
    /// `recv_split_by_eof`.
    Split,
}

/// A receive effect, which [`Effects::effect`] performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `socket->recv` of arg bytes into the buffer's tail, with upstream's read-error
    /// handling and warning: the byte count, or `RECV_CLOSE`/`RECV_DONE`.
    Recv,
    /// `get_package_length` over the buffered bytes from byte arg.
    GetLength,
    Extend,
    /// `onPackage` of the buffer's `offset` bytes (the awaited package) from byte arg.
    Dispatch,
    Reduce,
    /// `buffer->split` on the EOF, each part to `onPackage`.
    Split,
    SslPending,
    /// `socket->skip_recv = 0`.
    ClearSkip,
    /// `swoole_pagesize()`.
    Page,
    LogMalformed,
    LogNoLength,
    /// arg: the packet length (length protocol).
    LogTooLarge,
    /// Make the close that follows a reset (`SO_LINGER` 0): the length loop holds bytes
    /// past a bad package's header that upstream, reading only the header, would have
    /// left unread, and closing over unread bytes resets.
    Reset,
}

/// `Op::Recv`: the connection closes (`SW_ERR`).
pub const RECV_CLOSE: i64 = -1;
/// `Op::Recv`: the read ends with `SW_OK` (would block, logged error, removed socket).
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

pub const SW_OK: i32 = 0;
pub const SW_ERR: i32 = -1;
const SW_CONTINUE: i32 = 1;
pub const SW_CLOSE: i32 = 3;
const BUFFER_SIZE_STD: usize = 8192;

/// The head of `swoole::String`, without its bytes ([`Effects::data`]).
#[derive(Clone, Copy)]
pub struct Buffer {
    pub length: usize,
    pub size: usize,
    pub offset: i64,
}

/// The `swoole::Protocol` fields a receive reads.
#[derive(Clone, Copy)]
pub struct Protocol {
    pub split_by_eof: bool,
    pub package_eof: [u8; 8],
    pub package_eof_len: u8,
    pub package_length_offset: u16,
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
    /// Bytes at the buffer's start already dispatched: the length loop moves past
    /// each package and removes them all at once (`settle`), not one memmove each.
    start: usize,
    effects: &'a mut E,
}

impl<E: Effects> Recv<'_, E> {
    fn op(&mut self, op: Op, arg: u64) -> i64 {
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
        self.effects.buffer().length
    }
    fn size(&self) -> usize {
        self.effects.buffer().size
    }
    fn offset(&self) -> i64 {
        self.effects.buffer().offset
    }
    fn set_length(&mut self, length: usize) {
        let offset = self.offset();
        self.effects.set_buffer(length, offset);
    }
    fn set_offset(&mut self, offset: i64) {
        let length = self.length();
        self.effects.set_buffer(length, offset);
    }
    /// `buffer->clear()`.
    fn clear(&mut self) {
        self.start = 0;
        self.effects.set_buffer(0, 0);
    }
    /// Bytes buffered past the dispatched ones.
    fn pending(&self) -> usize {
        self.length() - self.start
    }
    /// Removes the dispatched bytes, so the buffer starts at the next package, as
    /// upstream's leaves it, before the loop reads, fails or returns.
    /// A callback that closed the socket may have freed the buffer, which upstream
    /// leaves alone then, so nothing is removed.
    fn settle(&mut self) {
        if self.flag(FLAG_REMOVED) && !self.flag(FLAG_PAUSED) {
            self.start = 0;
        } else if self.start > 0 {
            let start = std::mem::take(&mut self.start);
            let offset = self.offset();
            self.op(Op::Reduce, start as u64);
            self.set_offset(offset);
        }
    }
    fn eof_len(&self) -> usize {
        usize::from(self.effects.protocol().package_eof_len)
    }
    fn max(&self) -> u32 {
        self.effects.protocol().package_max_length
    }
    /// Appends `n` received bytes; `false` when they overrun the allocation.
    fn grow(&mut self, n: i64) -> bool {
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
        let protocol = self.effects.protocol();
        protocol
            .package_eof
            .get(..usize::from(protocol.package_eof_len))
            .is_some_and(|eof| self.effects.data().ends_with(eof))
    }

    #[inline(always)]
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
            self.op(Op::ClearSkip, 0);
            Step::GetLength
        } else {
            Step::Recv
        };
        loop {
            at = match at {
                Step::Recv => {
                    self.settle();
                    if self.flag(FLAG_REMOVED) {
                        return SW_OK;
                    }
                    let free = self.size() - self.length();
                    if free == 0 {
                        return SW_ERR;
                    }
                    match self.op(Op::Recv, free as u64) {
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
                    if self.op(Op::Dispatch, self.start as u64) < 0 {
                        self.settle();
                        return SW_ERR;
                    }
                    // A closed socket stops here, as upstream's does. A paused one
                    // still delivers the whole packages already read: only a partial
                    // one can be left, and the rest of it is still in the kernel.
                    if self.flag(FLAG_REMOVED) && !self.flag(FLAG_PAUSED) {
                        self.settle();
                        return SW_OK;
                    }
                    self.set_wait(false);
                    let offset = usize::try_from(self.offset()).unwrap_or(usize::MAX);
                    if self.pending() > offset {
                        self.start += offset;
                        self.set_offset(0);
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
                Step::Wait => {
                    self.settle();
                    return SW_OK;
                }
            };
        }
    }

    /// Bytes up to and including the length field: `package_length_offset` plus `head`.
    fn header(&self, head: u32) -> usize {
        (u32::from(self.effects.protocol().package_length_offset) + head) as usize
    }

    /// The next package's length from the buffered bytes: dispatch it if it is all
    /// here, else read more (unless the socket left the reactor).
    #[inline(always)]
    fn get_length(&mut self, head: u32, drained: bool) -> Step {
        let packet = self.op(Op::GetLength, self.start as u64);
        let header = self.header(head);
        if packet <= 0 || packet > i64::from(self.max()) {
            // Failures and a short header report the buffer as upstream holds it.
            self.settle();
        }
        if packet < 0 {
            self.op(Op::LogMalformed, 0);
            return self.fail(header);
        }
        if packet == 0 {
            // The whole length header is here and holds no length.
            if self.pending() >= header {
                self.op(Op::LogNoLength, 0);
                return self.fail(header);
            }
            return self.more(drained);
        }
        if packet > i64::from(self.max()) {
            self.op(Op::LogTooLarge, packet.unsigned_abs());
            return self.fail(header);
        }
        if usize::try_from(packet).map_or(true, |packet| self.size() - self.start < packet) {
            self.settle();
            if usize::try_from(packet).map_or(true, |packet| self.size() < packet) {
                self.op(Op::Extend, packet.unsigned_abs());
            }
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
            self.op(Op::Reset, 0);
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

    /// The buffer holds the whole awaited package (`length >= offset` past `start`).
    fn complete(&self) -> bool {
        usize::try_from(self.offset()).is_ok_and(|offset| self.pending() >= offset)
    }

    fn eof_protocol(&mut self) -> i32 {
        let mut again = false;
        loop {
            let size = self.size() - self.length();
            match self.op(Op::Recv, size.min(BUFFER_SIZE_STD) as u64) {
                RECV_DONE => return SW_OK,
                RECV_CLOSE | 0 => return SW_ERR,
                n if n < 0 => return SW_ERR,
                n if !self.grow(n) => return SW_ERR,
                _ => {}
            }
            if self.length() < self.eof_len() {
                return SW_OK;
            }
            if self.effects.protocol().split_by_eof {
                match self.split_by_eof() {
                    SW_CONTINUE => again = true,
                    SW_CLOSE => return SW_ERR,
                    _ => return SW_OK,
                }
            } else if self.ends_with_eof() {
                let length = self.length();
                self.set_offset(length.cast_signed() as i64);
                if self.op(Op::Dispatch, 0) < 0 {
                    return SW_ERR;
                }
                if self.flag(FLAG_REMOVED) {
                    return SW_OK;
                }
                self.clear();
                if self.flag(FLAG_TLS) && self.op(Op::SslPending, 0) > 0 {
                    continue;
                }
                return SW_OK;
            }
            let (length, size, max) = (self.length(), self.size(), self.max());
            if length == max as usize {
                self.op(Op::LogTooLarge, length as u64);
                return SW_ERR;
            }
            if length == size {
                again = true;
                if size < max as usize {
                    let page = self.op(Op::Page, 0);
                    let Some(page) = usize::try_from(page).ok().filter(|&page| page > 0) else {
                        return SW_ERR;
                    };
                    let double = size.wrapping_mul(2);
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "upstream's uint32_t extend_size"
                    )]
                    let extend = double.wrapping_add(page - double % page) as u32;
                    self.op(Op::Extend, u64::from(extend.min(max)));
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
        let n = self.op(Op::Split, 0);
        if self.flag(FLAG_REMOVED) || n < 0 {
            return SW_CLOSE;
        }
        if n == 0 {
            return SW_CONTINUE;
        }
        if usize::try_from(n).is_ok_and(|n| n < self.length()) {
            self.op(Op::Reduce, n.unsigned_abs());
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
/// holds the socket's flags at entry and its `recv_wait` on return. Always inlined, with the
/// length loop: each caller's `mode` is a constant, and the loop shares its caller's frame.
#[inline(always)]
pub fn recv<E: Effects>(mode: Mode, head: u32, flags: &mut u32, effects: &mut E) -> i32 {
    let mut recv = Recv {
        flags: *flags,
        start: 0,
        effects,
    };
    if recv.length() > recv.size() {
        return SW_ERR;
    }
    let result = match mode {
        Mode::Length => recv.length_protocol(head),
        Mode::Eof => recv.eof_protocol(),
        Mode::Split => recv.split_by_eof(),
    };
    *flags = recv.flags;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake native side: scripted reads, a growable buffer and a log of effects.
    struct Native {
        mode: Mode,
        head: u32,
        flags: u32,
        protocol: Protocol,
        buffer: Buffer,
        storage: Vec<u8>,
        chunks: Vec<&'static [u8]>,
        /// `FLAG_REMOVED` and `FLAG_PAUSED`, as the socket reports them.
        gone: u32,
        /// What `gone` becomes at dispatch number `gone_at` (a callback closing or pausing).
        gone_on_dispatch: u32,
        gone_at: usize,
        dispatches: usize,
        wait: bool,
        events: Vec<String>,
    }

    impl Native {
        fn new(mode: Mode, size: usize, chunks: Vec<&'static [u8]>) -> Self {
            Native {
                mode,
                head: 1,
                flags: 0,
                protocol: Protocol {
                    split_by_eof: false,
                    package_eof: *b"\r\n\0\0\0\0\0\0",
                    package_eof_len: 2,
                    package_length_offset: 0,
                    package_max_length: 64,
                },
                buffer: Buffer {
                    length: 0,
                    size,
                    offset: 0,
                },
                storage: vec![0; size],
                chunks,
                gone: 0,
                gone_on_dispatch: 0,
                gone_at: 1,
                dispatches: 0,
                wait: false,
                events: Vec::new(),
            }
        }
        fn data(&self) -> &[u8] {
            &self.storage[..self.buffer.length]
        }
        fn apply(&mut self, op: Op, arg: u64) -> i64 {
            let arg = usize::try_from(arg).unwrap();
            let length = self.buffer.length;
            match op {
                Op::Recv => {
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
                Op::GetLength => {
                    self.events.push(format!("length:{}", length - arg));
                    self.data()[arg..].first().map_or(0, |&n| i64::from(n) + 1)
                }
                Op::Extend => {
                    self.events.push(format!("extend:{arg}"));
                    self.storage.resize(arg, 0);
                    self.buffer.size = arg;
                    0
                }
                Op::Dispatch => {
                    let end = arg + usize::try_from(self.buffer.offset).unwrap();
                    let packet = String::from_utf8_lossy(&self.storage[arg..end]).into_owned();
                    self.events.push(format!("package:{packet}"));
                    self.dispatches += 1;
                    if self.dispatches == self.gone_at {
                        self.gone |= self.gone_on_dispatch;
                    }
                    0
                }
                Op::Reduce => {
                    self.events.push(format!("reduce:{arg}"));
                    self.storage.copy_within(arg..length, 0);
                    self.buffer.length -= arg;
                    self.buffer.offset = 0;
                    0
                }
                Op::Split => {
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
                Op::Page => 4096,
                Op::ClearSkip => 0,
                _ => {
                    self.events.push(format!("{op:?}:{arg}"));
                    0
                }
            }
        }
    }

    impl Effects for Native {
        fn effect(&mut self, op: Op, arg: u64, flags: &mut u32) -> i64 {
            self.wait = *flags & FLAG_WAIT != 0;
            let result = self.apply(op, arg);
            *flags = self.gone | if self.wait { FLAG_WAIT } else { 0 };
            result
        }
        fn buffer(&self) -> Buffer {
            self.buffer
        }
        fn set_buffer(&mut self, length: usize, offset: i64) {
            self.buffer.length = length;
            self.buffer.offset = offset;
        }
        fn data(&self) -> &[u8] {
            Native::data(self)
        }
        fn protocol(&self) -> Protocol {
            self.protocol
        }
    }

    fn run(native: &mut Native) -> i32 {
        let mut flags = native.flags;
        let status = recv(native.mode, native.head, &mut flags, native);
        native.flags = flags;
        native.wait = flags & FLAG_WAIT != 0;
        status
    }

    #[test]
    fn length_protocol_reads_what_is_there_then_waits() {
        // Each chunk is one arrival: a short read means the kernel had no more.
        let mut native = Native::new(Mode::Length, 2, vec![b"\x03", b"abc\x01"]);
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
        let mut native = Native::new(Mode::Length, 16, vec![b"\x01a\x02bc\x03d"]);
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(
            native.events,
            [
                "recv:16",
                "length:7",
                "package:\x01a",
                "length:5",
                "package:\x02bc",
                "length:2",
                "reduce:5"
            ]
        );
        assert_eq!(native.data(), b"\x03d", "the partial package stays");
        assert!(native.wait, "and waits for its last two bytes");
    }

    #[test]
    fn length_protocol_closed_mid_buffer_stops_and_paused_delivers_the_rest() {
        let mut native = Native::new(Mode::Length, 16, vec![b"\x01a\x01b"]);
        native.gone_on_dispatch = FLAG_REMOVED;
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(native.events, ["recv:16", "length:4", "package:\x01a"]);

        // Closed by a later package's callback: the buffer may be gone, so nothing is
        // removed from it, as upstream leaves it.
        let mut native = Native::new(Mode::Length, 16, vec![b"\x01a\x01b\x01c"]);
        native.gone_on_dispatch = FLAG_REMOVED;
        native.gone_at = 2;
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(
            native.events,
            [
                "recv:16",
                "length:6",
                "package:\x01a",
                "length:4",
                "package:\x01b"
            ]
        );

        let mut native = Native::new(Mode::Length, 16, vec![b"\x01a\x01b\x02", b"c"]);
        native.gone_on_dispatch = FLAG_REMOVED | FLAG_PAUSED;
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(
            native.events,
            [
                "recv:16",
                "length:5",
                "package:\x01a",
                "length:3",
                "package:\x01b",
                "length:1",
                "reduce:4"
            ],
            "no read while paused"
        );
        assert!(native.wait, "the partial package waits for resume");
    }

    #[test]
    fn length_protocol_dispatches_back_to_back_packages() {
        let mut native = Native::new(Mode::Length, 8, vec![b"\x10"]);
        native.storage[..3].copy_from_slice(b"\x01a\x00");
        native.buffer.length = 3;
        native.flags = FLAG_SKIP;
        assert_eq!(run(&mut native), SW_OK);
        assert_eq!(
            native.events,
            ["length:3", "package:\x01a", "length:1", "package:\0"]
        );
        assert_eq!(
            (native.buffer.length, native.buffer.offset),
            (0, 0),
            "cleared"
        );
        native.events.clear();
        native.protocol.package_max_length = 4;
        assert_eq!(run(&mut native), SW_ERR);
        assert_eq!(native.events, ["recv:8", "length:1", "LogTooLarge:17"]);
    }

    #[test]
    fn a_bad_package_with_bytes_past_its_header_closes_with_a_reset() {
        let mut native = Native::new(Mode::Length, 8, vec![b"\x10abc"]);
        native.protocol.package_max_length = 4;
        assert_eq!(run(&mut native), SW_ERR);
        assert_eq!(
            native.events,
            ["recv:8", "length:4", "LogTooLarge:17", "Reset:0"]
        );
    }

    #[test]
    fn length_protocol_stops_on_removed_and_dynamic_zero() {
        let mut native = Native::new(Mode::Length, 8, vec![b"\x01"]);
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
        let mut native = Native::new(Mode::Eof, 4, vec![b"abcd", b"e\r\n"]);
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
        let mut native = Native::new(Mode::Eof, 16, vec![b"a\r\nb\r\nc"]);
        native.protocol.split_by_eof = true;
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
        let mut native = Native::new(Mode::Eof, 2, vec![]);
        native.buffer.length = 3;
        assert_eq!(run(&mut native), SW_ERR);
        assert_eq!(native.events, Vec::<String>::new());
    }

    /// The C ABI gate's last receive (`tests/modules/framing/gate.cc`): a length
    /// header past the allocation fails before any effect.
    #[test]
    fn rejects_a_header_the_buffer_cannot_hold() {
        let mut native = Native::new(Mode::Length, 1, vec![b"\x01"]);
        native.protocol.package_length_offset = 3;
        assert_eq!(run(&mut native), SW_ERR);
        assert_eq!(native.events, Vec::<String>::new());
    }
}
