//! Coroutine packet framing directly over the socket's record and installed host.
/// `SW_BUFFER_SIZE_BIG`: the most one EOF-protocol `recv` asks for.
use swoole_rs_corebase::CCast;
const BIG: usize = 65536;

/// A view of the native read String, refreshed after every resize.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ReadBuffer {
    pub length: usize,
    pub size: usize,
    pub offset: libc::off_t,
    pub str: *mut u8,
}

/// Why a packet receive gave up, for the host's diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The protocol could not read a length from the header.
    LengthNotFound,
    /// The header's length exceeds `package_max_length`.
    TooLarge,
    /// The buffer filled to `package_max_length` with no EOF.
    NoEof,
}

/// What a sync of the read String does before it refreshes the view: nothing, or it
/// stores the view's length and offset, then grows the String to a size or drops that
/// many leading bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferSync {
    Get,
    Store,
    Extend(usize),
    Reduce(usize),
}

struct Packet {
    socket: crate::co_socket::Co,
    view: ReadBuffer,
    protocol: crate::co_socket::Protocol,
    length_check: bool,
    eof_check: bool,
}

impl Packet {
    fn buffer(&self) -> ReadBuffer {
        self.view
    }
    fn set_length(&mut self, length: usize) {
        self.view.length = length;
    }
    fn set_offset(&mut self, offset: usize) {
        self.view.offset = offset.c_cast::<libc::off_t>();
    }
    fn clear(&mut self) {
        self.set_length(0);
        self.set_offset(0);
    }
    fn sync(&mut self, sync: BufferSync) {
        (crate::co_io::host().read_buffer)(self.socket, &mut self.view, sync);
    }
    fn receive(&mut self, at: usize, length: usize, all: bool) -> isize {
        self.sync(BufferSync::Store);
        // SAFETY: the requested free space in the read String the view refreshed,
        // which the receive borrows.
        let buf =
            unsafe { core::slice::from_raw_parts_mut(self.view.str.wrapping_add(at), length) };
        (crate::co_io::host().recv)(self.socket, buf, all)
    }
    fn recv(&mut self, at: usize, length: usize) -> isize {
        self.receive(at, length, false)
    }
    fn fail(&mut self, kind: Failure, length: isize) {
        self.sync(BufferSync::Store);
        (crate::co_io::host().packet_fail)(self.socket, kind, length);
    }
    fn timed_out(&self) -> bool {
        self.socket.err_code() == libc::ETIMEDOUT
    }
    fn extend(&mut self, size: usize) {
        self.sync(BufferSync::Extend(size));
    }

    /// `recv_packet_with_length_protocol`.
    fn length_protocol(&mut self) -> isize {
        let protocol = self.protocol;
        let mut header_len =
            u32::from(protocol.package_length_offset) + u32::from(protocol.package_length_size);
        let buffered = self.buffer().length;
        // A custom package_length_func has neither a size nor a type.
        let mut known = buffered > 0
            && (buffered >= header_len as usize
                || protocol.package_length_size == 0 && protocol.package_length_type == 0);
        loop {
            if !known {
                let buffer = self.buffer();
                let received = self.recv(
                    buffer.length,
                    (header_len as usize).wrapping_sub(buffer.length),
                );
                if received <= 0 {
                    return received;
                }
                self.set_length(buffer.length + received.unsigned_abs());
            }
            known = false;
            let buffer = self.buffer();
            let mut header = 0;
            // It reads the initialized prefix and writes the header length.
            let packet = (crate::co_io::host().package_length)(
                self.socket,
                buffer.length.c_cast::<u32>(),
                &mut header,
            );
            if packet < 0 {
                self.fail(Failure::LengthNotFound, packet);
                return 0;
            }
            if packet == 0 {
                if header != 0 {
                    header_len = header;
                }
                continue;
            }
            if packet.unsigned_abs() > protocol.package_max_length as usize {
                self.clear();
                self.fail(Failure::TooLarge, packet);
                return -1;
            }
            let wanted = packet.unsigned_abs();
            if wanted <= buffer.length {
                self.set_offset(wanted);
                return packet;
            }
            if wanted > buffer.size {
                self.extend(wanted);
            }
            let buffer = self.buffer();
            let mut received = self.receive(buffer.length, wanted - buffer.length, true);
            if received > 0 {
                let length = buffer.length + received.unsigned_abs();
                self.set_length(length);
                if length == wanted {
                    self.set_offset(wanted);
                    return packet;
                }
                received = if self.timed_out() { -1 } else { 0 };
            }
            return received;
        }
    }

    /// `recv_packet_with_eof_protocol`.
    fn eof_protocol(&mut self) -> isize {
        let protocol = self.protocol;
        let eof = protocol
            .package_eof
            .get(..usize::from(protocol.package_eof_len))
            .unwrap_or(&protocol.package_eof);
        let maximum = protocol.package_max_length as usize;
        let mut buffered = self.buffer().length > 0;
        loop {
            if !buffered {
                let buffer = self.buffer();
                let received = self.recv(
                    buffer.length,
                    buffer.size.wrapping_sub(buffer.length).min(BIG),
                );
                if received <= 0 {
                    self.clear();
                    return received;
                }
                let length = buffer.length + received.unsigned_abs();
                self.set_length(length);
                if length < eof.len() {
                    continue;
                }
            }
            buffered = false;
            let buffer = self.buffer();
            if let Some(found) = find(&buffer, eof) {
                let offset = found + eof.len();
                self.set_offset(offset);
                return offset.cast_signed();
            }
            if buffer.length == maximum {
                self.clear();
                self.fail(Failure::NoEof, 0);
                return -1;
            }
            if buffer.length == buffer.size && buffer.size < maximum {
                self.extend((buffer.size * 2).min(maximum));
            }
        }
    }
}

/// `swoole_strnpos` over the buffered bytes, whose length it takes as `uint32_t`.
fn find(buffer: &ReadBuffer, needle: &[u8]) -> Option<usize> {
    let length = buffer.length.c_cast::<u32>() as usize;
    if needle.is_empty() || needle.len() > length {
        return None;
    }
    // SAFETY: the buffer's first `length` bytes are initialized.
    let haystack = unsafe { core::slice::from_raw_parts(buffer.str, length) };
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// `Socket::recv_packet`, including availability, timer and buffer allocation.
#[must_use]
pub fn recv_packet(socket: crate::co_socket::Co, timeout: f64) -> isize {
    use crate::co_io::{Timer, available};
    use crate::shutdown::READ;
    if !available(socket, READ) {
        return -1;
    }
    let timeout = if timeout == 0.0 {
        socket.timeout(crate::transfer::timeout_kind(READ))
    } else {
        timeout
    };
    let mut timer = Timer::new(socket, READ, timeout);
    if !timer.start() {
        return 0;
    }
    // The settings, read before any native effect may yield.
    let mut packet = Packet {
        socket,
        view: ReadBuffer {
            length: 0,
            size: 0,
            offset: 0,
            str: core::ptr::null_mut(),
        },
        protocol: socket.protocol(),
        length_check: socket.open_length_check(),
        eof_check: socket.open_eof_check(),
    };
    packet.sync(BufferSync::Get);
    let offset = packet.buffer().offset;
    if offset > 0 {
        packet.sync(BufferSync::Reduce(usize::try_from(offset).unwrap_or(0)));
    }
    let received = if packet.length_check {
        packet.length_protocol()
    } else if packet.eof_check {
        packet.eof_protocol()
    } else {
        let received = packet.recv(0, packet.buffer().size);
        if received > 0 {
            packet.set_length(received.unsigned_abs());
            packet.set_offset(received.unsigned_abs());
        }
        received
    };
    if received <= 0 && !(packet.length_check && received < 0 && packet.timed_out()) {
        packet.clear();
    }
    packet.sync(BufferSync::Store);
    timer.stop();
    received
}

#[cfg(test)]
mod ported;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::buffered::recv_buffered;
    use core::ffi::c_void;
    use std::collections::VecDeque;

    /// A socket whose `recv` hands out `chunks` in order, then 0 (EOF).
    struct Fake {
        storage: Vec<u8>,
        buffer: ReadBuffer,
        chunks: VecDeque<Vec<u8>>,
        failures: Vec<super::Failure>,
        timed_out: bool,
        gets: usize,
        receives: usize,
    }

    unsafe fn fake<'a>(context: *mut c_void) -> &'a mut Fake {
        unsafe { &mut *context.cast::<Fake>() }
    }

    unsafe extern "C" fn recv(context: *mut c_void, at: *mut u8, length: usize) -> isize {
        let fake = unsafe { fake(context) };
        fake.receives += 1;
        let Some(mut chunk) = fake.chunks.pop_front() else {
            return 0;
        };
        if chunk.len() > length {
            fake.chunks.push_front(chunk.split_off(length));
        }
        unsafe { at.copy_from_nonoverlapping(chunk.as_ptr(), chunk.len()) };
        chunk.len().c_cast::<isize>()
    }

    unsafe extern "C" fn recv_all(context: *mut c_void, at: *mut u8, length: usize) -> isize {
        let mut total = 0;
        while total < length {
            let received = unsafe { recv(context, at.add(total), length - total) };
            if received <= 0 {
                break;
            }
            total += received.c_cast::<usize>();
        }
        total.c_cast::<isize>()
    }

    /// A 4-byte big-endian body length after a 2-byte tag.
    unsafe extern "C" fn package_length(
        _: *mut c_void,
        at: *const u8,
        length: u32,
        _: *mut u32,
    ) -> isize {
        if length < 6 {
            return 0;
        }
        let bytes = unsafe { core::slice::from_raw_parts(at.add(2), 4) };
        6 + (u32::from_be_bytes(bytes.try_into().unwrap())).c_cast::<isize>()
    }

    unsafe fn fail(context: *mut c_void, kind: super::Failure) {
        unsafe { fake(context) }.failures.push(kind);
    }

    fn socket_for(
        fake: &mut Fake,
        length_check: bool,
        eof: &[u8],
        max_length: u32,
    ) -> crate::co_socket::TestSocket {
        crate::co_io::test_host::install();
        let mut socket = crate::co_socket::TestSocket::new();
        socket
            .0
            .resolve_context
            .set(core::ptr::from_mut(fake).cast());
        socket.0.open_length_check.set(length_check);
        socket.0.open_eof_check.set(!eof.is_empty());
        socket
            .0
            .err_code
            .set(if fake.timed_out { libc::ETIMEDOUT } else { 0 });
        let p = socket.0.protocol.get_mut();
        p.package_eof[..eof.len()].copy_from_slice(eof);
        p.package_eof_len = eof.len().c_cast::<u8>();
        p.package_length_type = b'N';
        p.package_length_size = 4;
        p.package_length_offset = 2;
        p.package_max_length = max_length;
        unsafe { (*socket.0.socket.get()).fields.timeouts.read = -1.0 };
        socket
    }

    pub(crate) fn read_buffer(
        socket: crate::co_socket::Co,
        view: &mut ReadBuffer,
        sync: super::BufferSync,
    ) {
        use super::BufferSync;
        let ctx = socket.resolve_context();
        let fake = unsafe { fake(ctx) };
        match sync {
            BufferSync::Get => fake.gets += 1,
            _ => fake.buffer = *view,
        }
        match sync {
            BufferSync::Extend(size) => {
                fake.storage.resize(size, 0);
                fake.buffer.str = fake.storage.as_mut_ptr();
                fake.buffer.size = size;
            }
            BufferSync::Reduce(size) => {
                fake.storage.copy_within(size..fake.buffer.length, 0);
                fake.buffer.length -= size;
                fake.buffer.offset = 0;
            }
            BufferSync::Get | BufferSync::Store => {}
        }
        *view = fake.buffer;
    }
    pub(crate) fn receive(socket: crate::co_socket::Co, buf: &mut [u8], all: bool) -> isize {
        let ctx = socket.resolve_context();
        let (at, size) = (buf.as_mut_ptr(), buf.len());
        if all {
            unsafe { recv_all(ctx, at, size) }
        } else {
            unsafe { recv(ctx, at, size) }
        }
    }
    pub(crate) fn get_length(socket: crate::co_socket::Co, size: u32, header: &mut u32) -> isize {
        let ctx = socket.resolve_context();
        let at = unsafe { fake(ctx) }.buffer.str;
        unsafe { package_length(ctx, at, size, header) }
    }
    pub(crate) fn failure(socket: crate::co_socket::Co, kind: super::Failure, _: isize) {
        let ctx = socket.resolve_context();
        unsafe { fail(ctx, kind) };
    }

    fn run(
        chunks: &[&[u8]],
        size: usize,
        length_check: bool,
        eof: &[u8],
        max_length: u32,
    ) -> (Vec<isize>, Box<Fake>) {
        let mut fake = Box::new(Fake {
            storage: vec![0; size],
            buffer: ReadBuffer {
                length: 0,
                size,
                offset: 0,
                str: core::ptr::null_mut(),
            },
            chunks: chunks.iter().map(|chunk| chunk.to_vec()).collect(),
            failures: Vec::new(),
            timed_out: false,
            gets: 0,
            receives: 0,
        });
        fake.buffer.str = fake.storage.as_mut_ptr();
        let mut socket = socket_for(&mut fake, length_check, eof, max_length);
        let mut results = Vec::new();
        loop {
            let result = recv_packet(socket.co(), 0.0);
            results.push(result);
            if result <= 0 {
                break;
            }
        }
        (results, fake)
    }

    #[test]
    fn length_protocol_reassembles_split_and_coalesced_packets() {
        let first = [&b"ab"[..], &[0, 0, 0, 3], b"xyz"].concat();
        let second = [&b"cd"[..], &[0, 0, 0, 1], b"q"].concat();
        let wire = [first.clone(), second.clone()].concat();
        let (results, _) = run(&[&wire[..4], &wire[4..12], &wire[12..]], 8, true, b"", 1024);
        assert_eq!(results, [9, 7, 0]);
    }

    #[test]
    fn length_protocol_rejects_oversized_packets() {
        let wire = [&b"ab"[..], &[0, 0, 4, 0]].concat();
        let (results, fake) = run(&[&wire], 64, true, b"", 1024);
        assert_eq!(results, [-1]);
        assert_eq!(fake.failures, [super::Failure::TooLarge]);
        assert_eq!(fake.buffer.length, 0);
    }

    #[test]
    fn eof_protocol_splits_on_the_delimiter_and_grows_the_buffer() {
        let (results, fake) = run(&[b"hello\r\nwor", b"ld\r\n"], 4, false, b"\r\n", 1024);
        assert_eq!(results, [7, 7, 0]);
        assert!(fake.buffer.size > 4);
    }

    #[test]
    fn eof_protocol_fails_at_the_maximum_without_a_delimiter() {
        let (results, fake) = run(&[b"0123456789abcdef"], 16, false, b"\r\n", 16);
        assert_eq!(results, [-1]);
        assert_eq!(fake.failures, [super::Failure::NoEof]);
    }

    #[test]
    fn partial_length_body_keeps_its_bytes_only_after_a_timeout() {
        // Upstream 0f96dc478: -1 after a timeout keeps the partial packet; 0 clears it.
        let wire = [&b"ab"[..], &[0, 0, 0, 8], b"part"].concat();
        for (timed_out, expected, kept) in [(true, -1, 10), (false, 0, 0)] {
            let mut fake = Box::new(Fake {
                storage: vec![0; 64],
                buffer: ReadBuffer {
                    length: 0,
                    size: 64,
                    offset: 0,
                    str: core::ptr::null_mut(),
                },
                chunks: VecDeque::from([wire.clone()]),
                failures: Vec::new(),
                timed_out,
                gets: 0,
                receives: 0,
            });
            fake.buffer.str = fake.storage.as_mut_ptr();
            let mut socket = socket_for(&mut fake, true, b"", 1024);
            assert_eq!(recv_packet(socket.co(), 0.0), expected);
            assert_eq!(fake.buffer.length, kept);
        }
    }

    #[test]
    fn raw_receive_returns_each_read() {
        let (results, fake) = run(&[b"abc", b"de"], 16, false, b"", 1024);
        assert_eq!(results, [3, 2, 0]);
        assert_eq!(fake.buffer.length, 0);
    }
    /// The socket points at the fake, so the caller makes it once the box has stopped
    /// moving: a move reasserts the box's unique ownership and ends that pointer.
    fn buffered_fake() -> Box<Fake> {
        let mut fake = Box::new(Fake {
            storage: vec![0; 8],
            buffer: ReadBuffer {
                length: 0,
                size: 8,
                offset: 0,
                str: core::ptr::null_mut(),
            },
            chunks: VecDeque::from([b"abcdefghij".to_vec()]),
            failures: vec![],
            timed_out: false,
            gets: 0,
            receives: 0,
        });
        fake.buffer.str = fake.storage.as_mut_ptr();
        fake
    }

    #[test]
    fn buffered_record_reads_publish_the_buffer_across_receives() {
        let mut fake = buffered_fake();
        let mut socket = socket_for(&mut fake, false, b"", 1024);
        let mut out = [0; 4];
        for (length, expected) in [(3, &b"abc"[..]), (4, &b"defg"[..]), (4, &b"hij"[..])] {
            assert_eq!(
                recv_buffered(socket.co(), &mut out[..length]),
                expected.len().c_cast::<isize>()
            );
            assert_eq!(&out[..expected.len()], expected);
        }
        assert_eq!(
            (
                fake.gets,
                fake.receives,
                fake.buffer.length,
                fake.buffer.offset
            ),
            (3, 2, 2, 2)
        );
    }

    #[test]
    fn unavailable_record_reads_do_not_allocate_or_receive() {
        let mut fake = buffered_fake();
        let mut socket = socket_for(&mut fake, false, b"", 1024);
        socket.0.sock_fd.set(-1);
        let mut out = [0; 1];
        assert_eq!(recv_buffered(socket.co(), &mut out), -1);
        assert_eq!(recv_packet(socket.co(), 0.0), -1);
        assert_eq!(socket.0.err_code.get(), libc::EBADF);
        assert_eq!((fake.gets, fake.receives), (0, 0));
    }

    #[test]
    fn packet_timer_failure_precedes_buffer_allocation() {
        let mut fake = buffered_fake();
        let mut socket = socket_for(&mut fake, false, b"", 1024);
        crate::co_io::test_host::TIMER_OK.set(false);
        assert_eq!(recv_packet(socket.co(), 1.0), 0);
        assert_eq!((fake.gets, fake.receives), (0, 0));
        crate::co_io::test_host::TIMER_OK.set(true);
        assert_eq!(recv_packet(socket.co(), 1.0), 8);
        assert_eq!(
            (
                fake.gets,
                fake.receives,
                fake.buffer.length,
                fake.buffer.offset
            ),
            (1, 1, 8, 8)
        );
        assert_eq!(recv_packet(socket.co(), 1.0), 2);
        assert_eq!(
            (&fake.storage[..2], fake.buffer.length, fake.buffer.offset),
            (&b"ij"[..], 2, 2)
        );
    }
}
