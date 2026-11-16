//! `Protocol::recv_with_length_protocol`, `recv_with_eof_protocol` and
//! `recv_split_by_eof`, each one call. The length loop reads greedily: one `recv` of
//! the buffer's free space, then every whole package in it, where upstream reads the
//! length header and then exactly the rest of the package, a `recv` each. The caller
//! holds the receive buffer (`swoole::String`) and the protocol: the loop reads and
//! sets them through [`Effects`], which performs each socket, callback, allocation or
//! log effect through its methods.

/// The socket, buffer and protocol of one receive. The loop holds nothing across
/// an effect: it reads the buffer and protocol again after each one.
///
/// Each effect applies [`FLAG_WAIT`] from `flags` to the socket first, and refreshes
/// `flags` from the socket after, except after a read or a length that succeeded.
pub trait Effects {
    /// Performs `op`, which reports nothing.
    fn effect(&mut self, op: Op, arg: usize, flags: &mut u32);
    /// `socket->recv` of at most `size` bytes into the buffer's tail, with upstream's
    /// read-error handling and warning.
    fn recv(&mut self, size: usize, flags: &mut u32) -> Received;
    /// `get_package_length` over the buffered bytes from byte `from`: `None` for
    /// malformed bytes, `Some(0)` while the length header is incomplete.
    fn length(&mut self, from: usize, flags: &mut u32) -> Option<usize>;
    /// `onPackage` of the buffer's `offset` bytes (the awaited package) from byte
    /// `from`: false fails the receive.
    fn dispatch(&mut self, from: usize, flags: &mut u32) -> bool;
    /// `buffer->split` on the EOF, each part to `onPackage`: the bytes handed on, or
    /// `None` when a part failed.
    fn split(&mut self, flags: &mut u32) -> Option<usize>;
    /// TLS holds decrypted bytes the kernel no longer reports.
    fn ssl_pending(&mut self, flags: &mut u32) -> bool;
    /// `swoole_pagesize()`.
    fn page(&mut self, flags: &mut u32) -> usize;
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

/// A receive effect that reports nothing, which [`Effects::effect`] performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// arg: the size the buffer grows to.
    Extend,
    /// arg: the bytes removed from the buffer's start.
    Reduce,
    /// `socket->skip_recv = 0`.
    ClearSkip,
    LogMalformed,
    LogNoLength,
    /// arg: the packet length (length protocol).
    LogTooLarge,
    /// Make the close that follows a reset (`SO_LINGER` 0): the length loop holds bytes
    /// past a bad package's header that upstream, reading only the header, would have
    /// left unread, and closing over unread bytes resets.
    Reset,
}

/// What [`Effects::recv`] read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Received {
    /// Bytes appended to the buffer; 0 when the peer closed.
    Bytes(usize),
    /// The connection closes (`SW_ERR`).
    Close,
    /// The read ends with `SW_OK` (would block, logged error, removed socket).
    Done,
}
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

/// A receive's result: upstream's `SW_OK`, `SW_ERR`, `SW_CONTINUE` and `SW_CLOSE`, as
/// the native caller reads them. Only [`Mode::Split`] continues or closes.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok = 0,
    Err = -1,
    Continue = 1,
    Close = 3,
}
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
    fn op(&mut self, op: Op, arg: usize) {
        self.effects.effect(op, arg, &mut self.flags);
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
            self.op(Op::Reduce, start);
            self.set_offset(offset);
        }
    }
    fn eof_len(&self) -> usize {
        usize::from(self.effects.protocol().package_eof_len)
    }
    fn max(&self) -> u32 {
        self.effects.protocol().package_max_length
    }
    /// Reads at most `size` bytes onto the buffer: the count, or the receive's end
    /// when the read ends it (the peer closed, or bytes overran the allocation).
    #[inline(always)]
    fn read(&mut self, size: usize) -> Result<usize, Status> {
        let n = match self.effects.recv(size, &mut self.flags) {
            Received::Done => return Err(Status::Ok),
            Received::Close | Received::Bytes(0) => return Err(Status::Err),
            Received::Bytes(n) => n,
        };
        let length = self.length();
        if n > self.size() - length {
            return Err(Status::Err);
        }
        self.set_length(length + n);
        Ok(n)
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
    fn length_protocol(&mut self, head: u32) -> Status {
        if self.flag(FLAG_DYNAMIC) && head == 0 {
            return Status::Err;
        }
        // A length header the buffer can never hold fails before any read.
        if self.header(head) > self.size() {
            return Status::Err;
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
                        return Status::Ok;
                    }
                    let free = self.size() - self.length();
                    if free == 0 {
                        return Status::Err;
                    }
                    match self.read(free) {
                        Ok(n) => drained = n < free,
                        Err(status) => return status,
                    }
                    if !self.flag(FLAG_WAIT) {
                        Step::GetLength
                    } else if self.complete() {
                        Step::Dispatch
                    } else if self.flag(FLAG_TLS) {
                        Step::Recv
                    } else {
                        return Status::Ok;
                    }
                }
                Step::GetLength => self.get_length(head, drained),
                Step::Dispatch => {
                    if !self.effects.dispatch(self.start, &mut self.flags) {
                        self.settle();
                        return Status::Err;
                    }
                    // A closed socket stops here, as upstream's does. A paused one
                    // still delivers the whole packages already read: only a partial
                    // one can be left, and the rest of it is still in the kernel.
                    if self.flag(FLAG_REMOVED) && !self.flag(FLAG_PAUSED) {
                        self.settle();
                        return Status::Ok;
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
                            return Status::Ok;
                        }
                        Step::Recv
                    }
                }
                Step::Fail => return Status::Err,
                Step::Wait => {
                    self.settle();
                    return Status::Ok;
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
        let packet = self.effects.length(self.start, &mut self.flags);
        let max = self.max() as usize;
        if packet.is_none_or(|packet| packet == 0 || packet > max) {
            // Failures and a short header report the buffer as upstream holds it.
            self.settle();
        }
        let Some(packet) = packet else {
            self.op(Op::LogMalformed, 0);
            return self.fail(head);
        };
        if packet == 0 {
            // The whole length header is here and holds no length.
            if self.pending() >= self.header(head) {
                self.op(Op::LogNoLength, 0);
                return self.fail(head);
            }
            return self.more(drained);
        }
        if packet > max {
            self.op(Op::LogTooLarge, packet);
            return self.fail(head);
        }
        if self.size() - self.start < packet {
            self.settle();
            if self.size() < packet {
                self.op(Op::Extend, packet);
            }
        }
        self.set_wait(true);
        self.set_offset(packet.cast_signed() as i64);
        if self.complete() {
            Step::Dispatch
        } else {
            self.more(drained)
        }
    }

    /// A bad package closes the connection; with bytes past its length header
    /// buffered, the close resets, as upstream's does over the bytes it never read.
    /// Only a failure reads the header's size, which the protocol holds.
    fn fail(&mut self, head: u32) -> Step {
        if self.length() > self.header(head) {
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

    fn eof_protocol(&mut self) -> Status {
        let mut again = false;
        loop {
            let size = self.size() - self.length();
            if let Err(status) = self.read(size.min(BUFFER_SIZE_STD)) {
                return status;
            }
            if self.length() < self.eof_len() {
                return Status::Ok;
            }
            if self.effects.protocol().split_by_eof {
                match self.split_by_eof() {
                    Status::Continue => again = true,
                    Status::Close => return Status::Err,
                    Status::Ok | Status::Err => return Status::Ok,
                }
            } else if self.ends_with_eof() {
                let length = self.length();
                self.set_offset(length.cast_signed() as i64);
                if !self.effects.dispatch(0, &mut self.flags) {
                    return Status::Err;
                }
                if self.flag(FLAG_REMOVED) {
                    return Status::Ok;
                }
                self.clear();
                if self.flag(FLAG_TLS) && self.effects.ssl_pending(&mut self.flags) {
                    continue;
                }
                return Status::Ok;
            }
            let (length, size, max) = (self.length(), self.size(), self.max());
            if length == max as usize {
                self.op(Op::LogTooLarge, length);
                return Status::Err;
            }
            if length == size {
                again = true;
                if size < max as usize {
                    let page = self.effects.page(&mut self.flags);
                    if page == 0 {
                        return Status::Err;
                    }
                    let double = size.wrapping_mul(2);
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "upstream's uint32_t extend_size"
                    )]
                    let extend = double.wrapping_add(page - double % page) as u32;
                    self.op(Op::Extend, extend.min(max) as usize);
                }
            }
            if !again {
                return Status::Ok;
            }
        }
    }

    fn split_by_eof(&mut self) -> Status {
        if self.length() < self.eof_len() {
            return Status::Continue;
        }
        let n = self.effects.split(&mut self.flags);
        let Some(n) = n.filter(|_| !self.flag(FLAG_REMOVED)) else {
            return Status::Close;
        };
        if n == 0 {
            return Status::Continue;
        }
        if n < self.length() {
            self.op(Op::Reduce, n);
            let offset = self.length().saturating_sub(self.eof_len());
            self.set_offset(offset.cast_signed() as i64);
        } else {
            self.clear();
        }
        // A TLS socket may hold decrypted bytes the kernel no longer reports.
        if self.flag(FLAG_TLS) {
            Status::Continue
        } else {
            Status::Ok
        }
    }
}

/// Runs one receive of `mode` and returns its result. `head` is the
/// package length size (`get_package_length_size` or the fixed size); `flags`
/// holds the socket's flags at entry and its `recv_wait` on return. Always inlined, with the
/// length loop: each caller's `mode` is a constant, and the loop shares its caller's frame.
#[inline(always)]
pub fn recv<E: Effects>(mode: Mode, head: u32, flags: &mut u32, effects: &mut E) -> Status {
    let mut recv = Recv {
        flags: *flags,
        start: 0,
        effects,
    };
    if recv.length() > recv.size() {
        return Status::Err;
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
        /// Runs `f` as the socket would: with `recv_wait` from `flags`, and
        /// `flags` refreshed after.
        fn sync<T>(&mut self, flags: &mut u32, f: impl FnOnce(&mut Self) -> T) -> T {
            self.wait = *flags & FLAG_WAIT != 0;
            let result = f(self);
            *flags = self.gone | if self.wait { FLAG_WAIT } else { 0 };
            result
        }
    }

    impl Effects for Native {
        fn effect(&mut self, op: Op, arg: usize, flags: &mut u32) {
            self.sync(flags, |n| {
                let length = n.buffer.length;
                match op {
                    Op::Extend => {
                        n.events.push(format!("extend:{arg}"));
                        n.storage.resize(arg, 0);
                        n.buffer.size = arg;
                    }
                    Op::Reduce => {
                        n.events.push(format!("reduce:{arg}"));
                        n.storage.copy_within(arg..length, 0);
                        n.buffer.length -= arg;
                        n.buffer.offset = 0;
                    }
                    Op::ClearSkip => {}
                    _ => n.events.push(format!("{op:?}:{arg}")),
                }
            });
        }
        fn recv(&mut self, size: usize, flags: &mut u32) -> Received {
            self.sync(flags, |n| {
                n.events.push(format!("recv:{size}"));
                if n.chunks.is_empty() {
                    return Received::Done;
                }
                let length = n.buffer.length;
                let chunk = n.chunks[0];
                let read = chunk.len().min(size);
                n.storage[length..length + read].copy_from_slice(&chunk[..read]);
                if read == chunk.len() {
                    n.chunks.remove(0);
                } else {
                    n.chunks[0] = &chunk[read..];
                }
                Received::Bytes(read)
            })
        }
        fn length(&mut self, from: usize, flags: &mut u32) -> Option<usize> {
            self.sync(flags, |n| {
                n.events.push(format!("length:{}", n.buffer.length - from));
                Some(n.data()[from..].first().map_or(0, |&b| usize::from(b) + 1))
            })
        }
        fn dispatch(&mut self, from: usize, flags: &mut u32) -> bool {
            self.sync(flags, |n| {
                let end = from + usize::try_from(n.buffer.offset).unwrap();
                let packet = String::from_utf8_lossy(&n.storage[from..end]).into_owned();
                n.events.push(format!("package:{packet}"));
                n.dispatches += 1;
                if n.dispatches == n.gone_at {
                    n.gone |= n.gone_on_dispatch;
                }
                true
            })
        }
        fn split(&mut self, flags: &mut u32) -> Option<usize> {
            self.sync(flags, |n| {
                let mut end = 0;
                while let Some(at) = n.storage[end..n.buffer.length]
                    .windows(2)
                    .position(|w| w == b"\r\n")
                {
                    let packet = String::from_utf8_lossy(&n.storage[end..end + at + 2]);
                    n.events.push(format!("package:{packet}"));
                    end += at + 2;
                }
                Some(end)
            })
        }
        fn ssl_pending(&mut self, flags: &mut u32) -> bool {
            self.sync(flags, |_| false)
        }
        fn page(&mut self, flags: &mut u32) -> usize {
            self.sync(flags, |_| 4096)
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

    fn run(native: &mut Native) -> Status {
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
        assert_eq!(run(&mut native), Status::Ok);
        assert_eq!(native.events, ["recv:2", "length:1", "extend:4"]);
        assert!(native.wait, "waits for the body");
        native.events.clear();
        assert_eq!(run(&mut native), Status::Ok);
        assert_eq!(native.events, ["recv:3", "package:\x03abc"]);
        assert_eq!(
            (native.buffer.length, native.buffer.offset),
            (0, 0),
            "cleared"
        );
        // The next call finds the next header and waits for its body.
        native.events.clear();
        assert_eq!(run(&mut native), Status::Ok);
        assert_eq!(native.events, ["recv:4", "length:1"]);
        assert!(native.wait, "waits for the next body");
        assert_eq!(native.buffer.offset, 2);
    }

    #[test]
    fn length_protocol_dispatches_every_package_one_read_brings() {
        let mut native = Native::new(Mode::Length, 16, vec![b"\x01a\x02bc\x03d"]);
        assert_eq!(run(&mut native), Status::Ok);
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
        assert_eq!(run(&mut native), Status::Ok);
        assert_eq!(native.events, ["recv:16", "length:4", "package:\x01a"]);

        // Closed by a later package's callback: the buffer may be gone, so nothing is
        // removed from it, as upstream leaves it.
        let mut native = Native::new(Mode::Length, 16, vec![b"\x01a\x01b\x01c"]);
        native.gone_on_dispatch = FLAG_REMOVED;
        native.gone_at = 2;
        assert_eq!(run(&mut native), Status::Ok);
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
        assert_eq!(run(&mut native), Status::Ok);
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
        assert_eq!(run(&mut native), Status::Ok);
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
        assert_eq!(run(&mut native), Status::Err);
        assert_eq!(native.events, ["recv:8", "length:1", "LogTooLarge:17"]);
    }

    #[test]
    fn a_bad_package_with_bytes_past_its_header_closes_with_a_reset() {
        let mut native = Native::new(Mode::Length, 8, vec![b"\x10abc"]);
        native.protocol.package_max_length = 4;
        assert_eq!(run(&mut native), Status::Err);
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
        assert_eq!(run(&mut native), Status::Ok);
        assert_eq!(native.events, Vec::<String>::new());
        native.flags = FLAG_DYNAMIC;
        native.head = 0;
        assert_eq!(run(&mut native), Status::Err);
        assert_eq!(native.events, Vec::<String>::new());
    }

    #[test]
    fn eof_protocol_grows_then_dispatches() {
        let mut native = Native::new(Mode::Eof, 4, vec![b"abcd", b"e\r\n"]);
        assert_eq!(run(&mut native), Status::Ok);
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
        assert_eq!(run(&mut native), Status::Ok);
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
        assert_eq!(run(&mut native), Status::Err);
        assert_eq!(native.events, Vec::<String>::new());
    }

    /// The C ABI gate's last receive (`tests/modules/framing/gate.cc`): a length
    /// header past the allocation fails before any effect.
    #[test]
    fn rejects_a_header_the_buffer_cannot_hold() {
        let mut native = Native::new(Mode::Length, 1, vec![b"\x01"]);
        native.protocol.package_length_offset = 3;
        assert_eq!(run(&mut native), Status::Err);
        assert_eq!(native.events, Vec::<String>::new());
    }
}
