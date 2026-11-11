//! `MessageBus`, the pipe protocol between reactors, workers and task workers.
//! Rust owns the receive buffer's bookkeeping, the chunked packets being
//! reassembled, and the read and write loops over the pipe's file descriptor.
//!
//! It also owns the bus's own records of its pipes (`init_pipe_socket`), and logs
//! what a read reports.
//!
//! Native code keeps allocation: the allocator may be Zend's, whose `emalloc` can
//! bail out (longjmp), so `Bus::receive` returns to its caller to allocate a
//! packet and is called again to resume. Freeing a packet calls its allocator's
//! `free`, which never bails out.
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void};
use std::hash::{BuildHasherDefault, Hasher};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use swoole_rs_corebase::{DataHead as BusHead, log};
use swoole_rs_reactor::{Socket, SocketRecord};

pub use Bus as MessageBus;

/// `SW_EVENT_DATA_*`.
const PTR: u8 = 1 << 1;
const CHUNK: u8 = 1 << 2;
const BEGIN: u8 = 1 << 3;
const END: u8 = 1 << 4;
const OBJ_PTR: u8 = 1 << 5;
/// `SW_WORKER_MAX_RECV_CHUNK_COUNT`.
const MAX_RECV_CHUNKS: u16 = 32;
const HEAD: usize = size_of::<BusHead>();
/// `SW_IPC_MSG_MIN`.
const IPC_MSG_MIN: u32 = 2048 - 40;
const _: () = assert!(HEAD == 40, "sizeof(DataHead)");
/// `swReturnCode`'s `SW_WAIT`.
const WAIT: c_int = 2;
/// `swReturnCode`'s `SW_REDUCE_SIZE`.
const REDUCE_SIZE: c_int = 7;
/// `SW_DEFAULT_ALIGNMENT`.
const ALIGNMENT: usize = size_of::<std::ffi::c_ulong>();

/// `BusRead::event`: what the caller does after `Bus::receive` returns.
pub const BUS_ALLOCATE: u8 = 1;
pub const BUS_CLOSED: u8 = 2;
pub const BUS_ABNORMAL: u8 = 3;
pub const BUS_READV_CLOSED: u8 = 4;

/// `swoole::PacketPtr`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BusPacket {
    pub length: usize,
    pub data: *mut c_char,
}

/// `swoole::Allocator`; only `free` is called from Rust.
#[repr(C)]
struct Allocator {
    malloc: *const c_void,
    calloc: *const c_void,
    realloc: *const c_void,
    free: unsafe extern "C" fn(*mut c_void),
}

/// One read's progress across the native allocations it asks for.
#[repr(C)]
#[derive(Debug)]
pub struct BusRead {
    /// Buffered reads: the datagram's length, kept across an allocation.
    pub received: isize,
    /// `BUS_ALLOCATE`: the bytes to allocate.
    pub size: usize,
    /// `BUS_ALLOCATE`: the block to reallocate, or null to allocate a new one.
    pub data: *mut c_void,
    /// `BUS_ALLOCATE` with `data`: the allocator that owns it.
    pub allocator: *const c_void,
    pub chunks: u16,
    /// The buffer holds the current chunk; set by `BUS_ALLOCATE`.
    pub resume: bool,
    pub event: u8,
}

struct Packet {
    data: *mut u8,
    length: usize,
    size: usize,
    allocator: *const Allocator,
}

impl Drop for Packet {
    fn drop(&mut self) {
        if self.data.is_null() {
            return;
        }
        // SAFETY: `allocator` outlives the bus (native allocators are static).
        let free = unsafe { (*self.allocator).free };
        // SAFETY: `data` came from this allocator and is owned by this packet.
        unsafe { free(self.data.cast()) };
    }
}

// Message ids are native counters: no SipHash per packet lookup.
#[derive(Default)]
struct IdHasher(u64);
impl Hasher for IdHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = self.0.wrapping_mul(257).wrapping_add(u64::from(byte));
        }
    }
    fn write_u64(&mut self, id: u64) {
        self.0 = id;
    }
    fn finish(&self) -> u64 {
        self.0.wrapping_mul(0x9e37_79b9_7f4a_7c15)
    }
}

pub struct Bus {
    buffer: *mut BusHead,
    buffer_size: usize,
    always_chunked: bool,
    packets: HashMap<u64, Packet, BuildHasherDefault<IdHasher>>,
    /// `pipe_sockets_`: by descriptor, this bus's own records of the pipes.
    pipe_sockets: Vec<*mut SocketRecord>,
    /// `id_generator_`: the counter that numbers each written message.
    next_id: Option<&'static AtomicU64>,
}

impl Drop for Bus {
    /// `~MessageBus`: the pipe records go, their descriptors stay open.
    fn drop(&mut self) {
        // Outside the hybrid (and in tests, which link no C++) a record has no
        // timers, buffers or loop: `Socket::free` is the record's deallocation.
        #[cfg(not(all(feature = "upstream-symbols", not(test))))]
        let free_socket: unsafe fn(*mut SocketRecord) = SocketRecord::free;
        for &socket in &self.pipe_sockets {
            // SAFETY: null or a record `init_pipe_socket` made, used by nothing else now.
            let Some(record) = (unsafe { Socket::from_ptr(socket) }) else {
                continue;
            };
            record.edit(|record| record.fd = -1);
            // SAFETY: as above.
            unsafe { free_socket(socket) };
        }
    }
}

#[cfg(all(feature = "upstream-symbols", not(test)))]
use swoole_rs_tls::upstream::socket_free as free_socket;
#[cfg(feature = "upstream-symbols")]
use swoole_rs_tls::upstream::socket_writev_sync as writev_sync;

/// `Socket::writev_sync` outside the hybrid: one writev(2).
///
/// # Safety
/// `socket` is a live record and `iov` denotes `count` readable iovecs.
#[cfg(not(feature = "upstream-symbols"))]
unsafe fn writev_sync(socket: *mut SocketRecord, iov: *const libc::iovec, count: usize) -> isize {
    let count = c_int::try_from(count).unwrap_or(c_int::MAX);
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    let fd = socket.fd();
    // SAFETY: as the caller promises.
    unsafe { libc::writev(fd, iov, count) }
}

fn aligned(size: usize) -> usize {
    size.wrapping_add(ALIGNMENT - 1) & !(ALIGNMENT - 1)
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `Socket::catch_read_error(errno) == SW_WAIT`.
fn waits(nobufs: &mut bool) -> bool {
    swoole_rs_reactor::catch_error(errno(), nobufs) == WAIT
}

impl Bus {
    fn new(buffer_size: usize) -> Self {
        Self {
            buffer: ptr::null_mut(),
            buffer_size,
            always_chunked: false,
            packets: HashMap::default(),
            pipe_sockets: Vec::new(),
            next_id: None,
        }
    }

    fn head(&self) -> BusHead {
        // SAFETY: reads only run on a bus whose buffer is set.
        unsafe { self.buffer.read() }
    }

    fn set_flags(&self, flags: u8) {
        // SAFETY: as `head`.
        unsafe { (*self.buffer).flags = flags };
    }

    fn data(&self) -> *mut u8 {
        self.buffer.cast::<u8>().wrapping_add(HEAD)
    }

    #[must_use]
    pub fn memory_size(&self) -> usize {
        self.buffer_size + self.packets.values().map(|p| p.size).sum::<usize>()
    }

    /// `MessageBus::get_packet`.
    #[must_use]
    pub fn packet(&self) -> BusPacket {
        let head = self.head();
        if head.flags & PTR != 0 {
            // SAFETY: `pass` stored a `BusPacket` at the start of the data.
            unsafe { self.data().cast::<BusPacket>().read_unaligned() }
        } else if head.flags & OBJ_PTR != 0 {
            self.packets.get(&head.msg_id).map_or(
                BusPacket {
                    length: 0,
                    data: ptr::null_mut(),
                },
                |p| BusPacket {
                    length: p.length,
                    data: p.data.cast(),
                },
            )
        } else {
            BusPacket {
                length: head.len as usize,
                data: self.data().cast(),
            }
        }
    }

    /// `MessageBus::pass`: the buffer points at `data` instead of holding it.
    pub fn pass(&self, info: &BusHead, data: *const c_char) {
        let mut head = *info;
        if info.len > 0 {
            head.flags = PTR;
            head.len = 16; // sizeof(PacketPtr)
            let packet = BusPacket {
                length: info.len as usize,
                data: data.cast_mut(),
            };
            // SAFETY: the buffer holds at least a header and a pointer pair.
            unsafe { self.data().cast::<BusPacket>().write_unaligned(packet) };
        }
        // SAFETY: as `head`.
        unsafe { self.buffer.write(head) };
    }

    /// `MessageBus::move_packet`: the current packet's block, now the caller's.
    pub fn move_packet(&mut self) -> *mut c_char {
        let id = self.head().msg_id;
        self.packets.get_mut(&id).map_or(ptr::null_mut(), |p| {
            std::mem::replace(&mut p.data, ptr::null_mut()).cast()
        })
    }

    /// `MessageBus::pop`: frees the current packet once its last chunk arrived.
    pub fn pop(&mut self) {
        let head = self.head();
        if head.flags & END != 0 {
            self.packets.remove(&head.msg_id);
        }
    }

    /// Hands the block `BUS_ALLOCATE` asked for to the current packet.
    ///
    /// # Safety
    /// The buffer is set; `data` holds `size` bytes from `allocator` (a
    /// `swoole::Allocator` that outlives the bus), which the packet now owns.
    pub unsafe fn adopt(&mut self, data: *mut c_void, size: usize, allocator: *const c_void) {
        let (data, allocator) = (data.cast::<u8>(), allocator.cast::<Allocator>());
        let packet = self.packets.entry(self.head().msg_id).or_insert(Packet {
            data: ptr::null_mut(),
            length: 0,
            size: 0,
            allocator,
        });
        // A reallocation already moved the old block.
        packet.data = data;
        packet.size = size;
    }

    /// `MessageBus::read` (stream pipes, `dgram == false`) and
    /// `MessageBus::read_with_buffer` (datagram pipes): >0 a whole packet, 0 wait
    /// (or allocate, see `state.event`), -1 an error.
    ///
    /// # Safety
    /// The buffer is set.
    unsafe fn read(
        &mut self,
        fd: c_int,
        dgram: bool,
        state: &mut BusRead,
        nobufs: &mut bool,
    ) -> isize {
        state.event = 0;
        loop {
            if state.resume {
                state.resume = false;
            } else {
                let n = if dgram {
                    // SAFETY: the buffer holds `buffer_size` writable bytes.
                    unsafe { libc::read(fd, self.buffer.cast(), self.buffer_size) }
                } else {
                    // SAFETY: the buffer starts with a writable header.
                    unsafe { libc::recv(fd, self.buffer.cast(), HEAD, libc::MSG_PEEK) }
                };
                if n < 0 {
                    return if waits(nobufs) { 0 } else { -1 };
                }
                if n == 0 {
                    state.event = BUS_CLOSED;
                    return -1;
                }
                let head = self.head();
                if dgram {
                    state.chunks = state.chunks.wrapping_add(1);
                    state.received = n;
                }
                if head.flags & CHUNK == 0 {
                    if dgram {
                        return n;
                    }
                    let length = (HEAD + head.len as usize).min(self.buffer_size);
                    // SAFETY: `length` is within the buffer.
                    return unsafe { libc::read(fd, self.buffer.cast(), length) };
                }
            }
            let head = self.head();
            let (buffer, payload, buffer_size) = (self.buffer, self.data(), self.buffer_size);
            let Some(packet) = self.packets.get_mut(&head.msg_id) else {
                if head.flags & BEGIN != 0 {
                    return Self::allocate(state, ptr::null_mut(), ptr::null(), head.len as usize);
                }
                state.event = BUS_ABNORMAL;
                if dgram {
                    return -1;
                }
                // Read the chunk's header from the pipe and discard it.
                // SAFETY: the buffer starts with a writable header.
                unsafe { libc::recv(fd, buffer.cast(), HEAD, 0) };
                return 0;
            };
            let n = if dgram {
                let count = state.received.cast_unsigned().saturating_sub(HEAD);
                let length = packet.length + count;
                if length > packet.size || packet.data.is_null() {
                    return Self::allocate(state, packet.data, packet.allocator, length);
                }
                let tail = packet.data.wrapping_add(packet.length);
                // SAFETY: the datagram's payload follows the header in the buffer, and
                // the packet's tail has room for it (checked above); they never overlap.
                unsafe { ptr::copy_nonoverlapping(payload, tail, count) };
                packet.length = length;
                state.received
            } else {
                let remain = (head.len as usize).wrapping_sub(packet.length);
                let iov = [
                    libc::iovec {
                        iov_base: buffer.cast(),
                        iov_len: HEAD,
                    },
                    libc::iovec {
                        iov_base: packet.data.wrapping_add(packet.length).cast(),
                        iov_len: (buffer_size - HEAD).min(remain),
                    },
                ];
                // SAFETY: both iovecs name writable memory: the buffer's header and
                // the packet's unfilled tail (at most `remain` bytes).
                let n = unsafe { libc::readv(fd, iov.as_ptr(), 2) };
                if n == 0 {
                    state.event = BUS_READV_CLOSED;
                    return -1;
                }
                if n < 0 && waits(nobufs) {
                    return 0;
                }
                if n > 0 {
                    packet.length = packet
                        .length
                        .wrapping_add(n.cast_unsigned().wrapping_sub(HEAD));
                }
                n
            };
            state.chunks = state.chunks.wrapping_add(1);
            let flags = self.head().flags;
            if flags & END != 0 {
                self.set_flags(flags | OBJ_PTR);
                return n;
            }
            if state.chunks >= MAX_RECV_CHUNKS {
                return 0;
            }
        }
    }

    fn allocate(
        state: &mut BusRead,
        data: *mut u8,
        allocator: *const Allocator,
        size: usize,
    ) -> isize {
        state.event = BUS_ALLOCATE;
        state.resume = true;
        state.data = data.cast();
        state.allocator = allocator.cast();
        state.size = aligned(size);
        0
    }

    /// `MessageBus::write`: one direct write, or chunks of at most the buffer's
    /// payload, halved down to `SW_IPC_MSG_MIN` while the pipe asks for less.
    ///
    /// `send` writes the iovecs to the pipe and never bails out; `reduce` says
    /// whether the failed write's errno asks for smaller chunks (`SW_REDUCE_SIZE`).
    ///
    /// # Safety
    /// `data` covers `head.len` bytes or is null.
    unsafe fn write(
        &self,
        head: &mut BusHead,
        data: *const u8,
        mut send: impl FnMut(&[libc::iovec]) -> isize,
        mut reduce: impl FnMut() -> bool,
    ) -> bool {
        let maximum = u32::try_from(self.buffer_size - HEAD).unwrap_or(u32::MAX);
        let Some(mut writer) = BusWriter::new(
            head.len,
            maximum,
            IPC_MSG_MIN,
            !data.is_null(),
            self.always_chunked,
        ) else {
            return false;
        };
        while writer.mode != COMPLETE {
            let chunk = writer.next_chunk();
            head.flags = chunk.flags;
            if chunk.mode == EMPTY {
                head.len = 0;
            }
            let iov = [
                libc::iovec {
                    iov_base: ptr::from_mut(head).cast(),
                    iov_len: HEAD,
                },
                libc::iovec {
                    iov_base: if data.is_null() {
                        ptr::null_mut()
                    } else {
                        data.wrapping_add(chunk.offset as usize).cast_mut().cast()
                    },
                    iov_len: chunk.length as usize,
                },
            ];
            let count = if chunk.mode == EMPTY { 1 } else { 2 };
            let sent = send(iov.get(..count).unwrap_or_default());
            // Chunks accept any nonnegative write; direct writes need every byte.
            let success = if chunk.mode == CHUNKS {
                sent >= 0
            } else {
                sent.cast_unsigned() == HEAD + chunk.length as usize
            };
            let smaller = !success && chunk.mode != EMPTY && reduce();
            if writer.complete(success, smaller) < 0 {
                return false;
            }
            head.flags = writer.flags;
        }
        true
    }
}

const EMPTY: u8 = 0;
const DIRECT: u8 = 1;
const CHUNKS: u8 = 2;
const COMPLETE: u8 = 3;

#[derive(Clone, Copy, Debug, Default)]
struct BusWriter {
    remaining: u32,
    offset: u32,
    maximum: u32,
    minimum: u32,
    flags: u8,
    mode: u8,
}
#[derive(Clone, Copy, Debug, Default)]
struct BusChunk {
    offset: u32,
    length: u32,
    flags: u8,
    mode: u8,
}

impl BusWriter {
    fn new(
        length: u32,
        maximum: u32,
        minimum: u32,
        has_payload: bool,
        always: bool,
    ) -> Option<Self> {
        if minimum == 0 || (maximum == 0 && has_payload && length != 0) {
            return None;
        }
        let mut writer = Self {
            remaining: length,
            maximum,
            minimum,
            ..Self::default()
        };
        if !has_payload || length == 0 {
            writer.remaining = 0;
        } else if !always && length <= maximum {
            writer.mode = DIRECT;
        } else {
            writer.mode = CHUNKS;
            writer.flags = CHUNK | BEGIN;
        }
        Some(writer)
    }

    fn next_chunk(&self) -> BusChunk {
        let length = if self.mode == CHUNKS {
            self.remaining.min(self.maximum)
        } else {
            self.remaining
        };
        BusChunk {
            offset: self.offset,
            length,
            flags: self.flags
                | if self.mode == CHUNKS && self.remaining <= self.maximum {
                    END
                } else {
                    0
                },
            mode: self.mode,
        }
    }

    fn complete(&mut self, success: bool, reduce: bool) -> i32 {
        if !success {
            if !reduce || self.maximum <= self.minimum {
                return -1;
            }
            self.maximum = self.minimum;
            if self.mode == DIRECT {
                self.mode = CHUNKS;
                self.flags = CHUNK | BEGIN;
            } else {
                self.flags &= !END;
            }
            return 1;
        }
        let chunk = self.next_chunk();
        self.flags = chunk.flags & !BEGIN;
        if self.mode == CHUNKS {
            self.remaining -= chunk.length;
            self.offset += chunk.length;
        } else {
            self.remaining = 0;
        }
        if self.remaining == 0 {
            self.mode = COMPLETE;
        }
        0
    }
}

/// `MessageBus`'s operations for the server and pools that hold one.
impl Bus {
    /// `new MessageBus()` with no buffer yet.
    #[must_use]
    pub fn create(buffer_size: usize) -> Box<Self> {
        Box::new(Self::new(buffer_size))
    }

    pub fn set_buffer_size(&mut self, size: usize) {
        self.buffer_size = size;
    }

    #[must_use]
    pub fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    pub fn set_always_chunked(&mut self) {
        self.always_chunked = true;
    }

    /// Sets the receive buffer, `buffer_size` bytes the caller allocated and frees,
    /// and zeroes its header; or forgets it (null).
    ///
    /// # Safety
    /// A non-null `buffer` holds `buffer_size` writable bytes and stays valid until
    /// it is replaced; the reads, packets and passes below need one set.
    pub unsafe fn set_buffer(&mut self, buffer: *mut c_void) {
        self.buffer = buffer.cast();
        if !buffer.is_null() {
            // SAFETY: the caller lends a buffer that starts with a writable header.
            unsafe { self.buffer.write(BusHead::default()) };
        }
    }

    /// The receive buffer, which starts with the current message's head.
    #[must_use]
    pub fn buffer(&self) -> *mut BusHead {
        self.buffer
    }

    /// Packets being reassembled.
    #[must_use]
    pub fn count(&self) -> usize {
        self.packets.len()
    }

    pub fn clear(&mut self) {
        self.packets.clear();
    }

    /// Reads from the pipe `fd`; see `Bus::read`. On `BUS_ALLOCATE` the caller
    /// allocates (or reallocates) `state.size` bytes, calls [`Bus::adopt`], and
    /// calls this again with the same `state`.
    ///
    /// # Safety
    /// The buffer is set.
    pub unsafe fn receive(
        &mut self,
        fd: c_int,
        dgram: bool,
        nobufs: &mut bool,
        state: &mut BusRead,
    ) -> isize {
        // SAFETY: the caller's contract.
        let n = unsafe { self.read(fd, dgram, state, nobufs) };
        let origin = if dgram {
            c"MessageBus::read_with_buffer"
        } else {
            c"MessageBus::read"
        };
        match state.event {
            BUS_CLOSED => log::warning(
                origin,
                format_args!("receive data from socket#{fd} returns 0"),
            ),
            BUS_READV_CLOSED => log::warning(
                origin,
                format_args!(
                    "receive pipeline data error, pipe_fd={fd}, reactor_id={}",
                    self.head().reactor_id
                ),
            ),
            BUS_ABNORMAL => log::error_log(
                /* SW_LOG_WARNING */ 4,
                /* SW_ERROR_SERVER_WORKER_ABNORMAL_PIPE_DATA */ 9102,
                origin,
                format_args!(
                    "abnormal pipeline data, msg_id={}, pipe_fd={fd}, reactor_id={}",
                    self.head().msg_id,
                    self.head().reactor_id
                ),
            ),
            _ => {}
        }
        n
    }

    /// `MessageBus::count_ids`: numbers each message written from `counter`, an
    /// atomic `sw_atomic_long_t` that buses in other processes may share.
    pub fn count_ids(&mut self, counter: &'static AtomicU64) {
        self.next_id = Some(counter);
    }

    /// `MessageBus::write`: numbers the message, then writes it through the event
    /// loop while one is available, else synchronously. False if the pipe failed.
    ///
    /// # Safety
    /// `socket` is a live record; `data` covers `head.len` bytes or is null.
    pub unsafe fn send(
        &self,
        socket: *mut SocketRecord,
        head: &mut BusHead,
        data: *const u8,
    ) -> bool {
        use swoole_rs_reactor::event_api;
        // `write` passes iovecs over `head` and the caller's bytes.
        let send = |iov: &[libc::iovec]| {
            if event_api::swoole_event_is_available() {
                // SAFETY: the caller's live record and the write's iovecs, on the
                // thread's live loop.
                unsafe { event_api::swoole_event_writev(socket.cast(), iov.as_ptr(), iov.len()) }
            } else {
                // SAFETY: the caller's live record and the write's iovecs.
                unsafe { writev_sync(socket, iov.as_ptr(), iov.len()) }
            }
        };
        let reduce = || {
            let error = errno();
            // SAFETY: the caller's live record.
            let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
                return false;
            };
            socket.edit(|record| {
                swoole_rs_reactor::catch_pipe_error(error, &mut record.kernel_nobufs)
            }) == REDUCE_SIZE
        };
        if let Some(counter) = self.next_id {
            head.msg_id = counter.fetch_add(1, Ordering::SeqCst);
        }
        // SAFETY: the caller's contract.
        unsafe { self.write(head, data, send, reduce) }
    }

    /// `MessageBus::init_pipe_socket`: this bus's own nonblocking record of the pipe
    /// `fd`, with no send-buffer limit.
    pub fn init_pipe_socket(&mut self, fd: c_int) {
        let Ok(index) = usize::try_from(fd) else {
            return;
        };
        if index >= self.pipe_sockets.len() {
            self.pipe_sockets.resize(index + 1, ptr::null_mut());
        }
        // `make_socket`: a new record, which the bus frees on drop.
        let socket = Box::leak(Box::new(SocketRecord::with_fd(fd, /* SW_FD_PIPE */ 3)));
        socket.output.buffer_size = u32::MAX;
        if socket.output.nonblock == 0 {
            swoole_rs_reactor::socket_upstream::socket_set_fd_option(socket, 1, -1);
        }
        if let Some(slot) = self.pipe_sockets.get_mut(index) {
            *slot = socket;
        }
    }

    /// `MessageBus::get_pipe_socket`: the record `init_pipe_socket` made for `fd`.
    #[must_use]
    pub fn pipe_socket(&self, fd: c_int) -> *mut SocketRecord {
        usize::try_from(fd)
            .ok()
            .and_then(|index| self.pipe_sockets.get(index).copied())
            .unwrap_or(ptr::null_mut())
    }
}

/// [`Bus::send`] for crates/process' `ProcessPool::send_message`, which cannot
/// depend on this crate.
///
/// # Safety
/// `socket` is a live record; `data` covers `head.len` bytes or is null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_write(
    bus: &Bus,
    socket: *mut SocketRecord,
    head: &mut BusHead,
    data: *const c_char,
) -> bool {
    // SAFETY: the caller's contract.
    unsafe { bus.send(socket, head, data.cast()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::RawFd;

    unsafe extern "C" fn free(data: *mut c_void) {
        unsafe { libc::free(data) };
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no socket options on a socketpair")]
    fn send_numbers_each_message_from_the_shared_counter() {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let mut pipe = Pipe::new(libc::SOCK_STREAM, 64, false);
        pipe.bus.count_ids(&COUNTER);
        let mut socket = SocketRecord::with_fd(pipe.fds[0], /* SW_FD_PIPE */ 3);
        for (expected, payload) in [(1, b"one".as_slice()), (2, b"two".as_slice())] {
            let mut head = BusHead {
                msg_id: 99,
                len: u32::try_from(payload.len()).unwrap(),
                fd: 7,
                ..BusHead::default()
            };
            assert!(unsafe { pipe.bus.send(&raw mut socket, &mut head, payload.as_ptr()) });
            assert_eq!(head.msg_id, expected);
            assert_eq!(pipe.read(false).as_deref(), Some(payload));
        }
        assert_eq!(COUNTER.load(Ordering::Relaxed), 3);
    }
    struct Static(Allocator);
    unsafe impl Sync for Static {}
    static ALLOCATOR: Static = Static(Allocator {
        malloc: ptr::null(),
        calloc: ptr::null(),
        realloc: ptr::null(),
        free,
    });

    struct Pipe {
        bus: Bus,
        buffer: Vec<u64>,
        fds: [RawFd; 2],
    }
    impl Pipe {
        fn new(kind: c_int, size: usize, always: bool) -> Self {
            let mut fds = [0; 2];
            assert_eq!(
                unsafe { libc::socketpair(libc::AF_UNIX, kind, 0, fds.as_mut_ptr()) },
                0
            );
            for fd in fds {
                let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
                let buffer = c_int::try_from(1 << 20).unwrap();
                for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
                    unsafe {
                        libc::setsockopt(
                            fd,
                            libc::SOL_SOCKET,
                            option,
                            ptr::from_ref(&buffer).cast(),
                            4,
                        )
                    };
                }
            }
            let mut bus = Bus::new(size);
            bus.always_chunked = always;
            let mut buffer = vec![0u64; size / 8];
            bus.buffer = buffer.as_mut_ptr().cast();
            Self { bus, buffer, fds }
        }
        fn write(&self, id: u64, payload: &[u8]) -> bool {
            let mut head = BusHead {
                msg_id: id,
                len: u32::try_from(payload.len()).unwrap(),
                fd: 7,
                ..BusHead::default()
            };
            let data = if payload.is_empty() {
                ptr::null()
            } else {
                payload.as_ptr()
            };
            let fd = self.fds[0];
            let send = |iov: &[libc::iovec]| unsafe {
                libc::writev(fd, iov.as_ptr(), c_int::try_from(iov.len()).unwrap())
            };
            unsafe { self.bus.write(&mut head, data, send, || false) }
        }
        /// Reads until a packet completes, allocating like the native facade.
        fn read(&mut self, dgram: bool) -> Option<Vec<u8>> {
            let mut state = BusRead {
                received: 0,
                size: 0,
                data: ptr::null_mut(),
                allocator: ptr::null(),
                chunks: 0,
                resume: false,
                event: 0,
            };
            let mut nobufs = false;
            loop {
                let n = unsafe { self.bus.read(self.fds[1], dgram, &mut state, &mut nobufs) };
                if state.event == BUS_ALLOCATE {
                    let data = unsafe { libc::realloc(state.data, state.size) };
                    let allocator = (&raw const ALLOCATOR.0).cast();
                    unsafe { self.bus.adopt(data, state.size, allocator) };
                    continue;
                }
                if n < 0 {
                    return None;
                }
                if n == 0 {
                    state.chunks = 0;
                    continue;
                }
                let packet = self.bus.packet();
                let bytes =
                    unsafe { std::slice::from_raw_parts(packet.data.cast::<u8>(), packet.length) };
                let bytes = bytes.to_vec();
                assert_eq!(self.bus.head().fd, 7);
                self.bus.pop();
                return Some(bytes);
            }
        }
    }
    impl Drop for Pipe {
        fn drop(&mut self) {
            for fd in self.fds {
                unsafe { libc::close(fd) };
            }
            assert_ne!(self.buffer.len(), 0);
        }
    }

    fn payload(length: usize) -> Vec<u8> {
        (0..length)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect()
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no socket options on a socketpair")]
    fn stream_round_trips_direct_chunked_and_empty_packets() {
        let mut pipe = Pipe::new(libc::SOCK_STREAM, 8192, false);
        for (id, length) in [(1, 100), (2, 8152), (3, 8153), (4, 200_000), (5, 0)] {
            let data = payload(length);
            assert!(pipe.write(id, &data));
            assert_eq!(pipe.read(false).unwrap(), data, "packet {id}");
        }
        assert!(pipe.bus.packets.is_empty());
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no datagram socketpair")]
    fn dgram_round_trips_always_chunked_packets() {
        let mut pipe = Pipe::new(libc::SOCK_DGRAM, 8192, true);
        for (id, length) in [(1, 10), (2, 100_000)] {
            let data = payload(length);
            assert!(pipe.write(id, &data));
            assert_eq!(pipe.read(true).unwrap(), data, "packet {id}");
        }
        assert!(pipe.bus.packets.is_empty());
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no socket options on a socketpair")]
    fn moved_packets_are_not_freed_and_memory_counts_reassembly() {
        let mut pipe = Pipe::new(libc::SOCK_STREAM, 4096, true);
        assert!(pipe.write(9, &payload(5000)));
        let mut state = BusRead {
            received: 0,
            size: 0,
            data: ptr::null_mut(),
            allocator: ptr::null(),
            chunks: 0,
            resume: false,
            event: 0,
        };
        let mut nobufs = false;
        assert_eq!(
            unsafe { pipe.bus.read(pipe.fds[1], false, &mut state, &mut nobufs) },
            0
        );
        assert_eq!((state.event, state.size), (BUS_ALLOCATE, 5000));
        let data = unsafe { libc::malloc(state.size) };
        let allocator = (&raw const ALLOCATOR.0).cast();
        unsafe { pipe.bus.adopt(data, state.size, allocator) };
        assert_eq!(pipe.bus.memory_size(), 4096 + 5000);
        assert!(unsafe { pipe.bus.read(pipe.fds[1], false, &mut state, &mut nobufs) } > 0);
        assert_eq!(pipe.bus.packet().length, 5000);
        let moved = pipe.bus.move_packet();
        assert_eq!(moved.cast::<c_void>(), data);
        assert!(pipe.bus.packet().data.is_null());
        pipe.bus.pop();
        unsafe { libc::free(moved.cast()) };
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no datagram socketpair")]
    fn unknown_chunks_and_closed_pipes_are_reported() {
        let mut pipe = Pipe::new(libc::SOCK_DGRAM, 4096, true);
        let head = BusHead {
            msg_id: 3,
            len: 10,
            flags: CHUNK | END,
            ..BusHead::default()
        };
        let sent = unsafe { libc::send(pipe.fds[0], ptr::from_ref(&head).cast(), HEAD, 0) };
        assert_eq!(sent.cast_unsigned(), HEAD);
        assert_eq!(pipe.read(true), None);
        assert_eq!(pipe.bus.packets.len(), 0);
    }

    #[test]
    fn pass_points_at_the_payload() {
        let mut buffer = [0u64; 16];
        let mut bus = Bus::new(128);
        bus.buffer = buffer.as_mut_ptr().cast();
        let data = b"hello";
        let info = BusHead {
            len: 5,
            fd: 3,
            ..BusHead::default()
        };
        bus.pass(&info, data.as_ptr().cast());
        let packet = bus.packet();
        assert_eq!(
            (packet.length, packet.data.cast_const()),
            (5, data.as_ptr().cast())
        );
        assert_eq!(
            (bus.head().flags, bus.head().len, bus.head().fd),
            (PTR, 16, 3)
        );
    }

    #[test]
    fn reduction_retries_current_chunk_and_preserves_begin_end() {
        let mut writer = BusWriter::new(2000, 4096, 1024, true, false).unwrap();
        assert_eq!(writer.next_chunk().mode, DIRECT);
        assert_eq!(writer.complete(false, true), 1);
        let first = writer.next_chunk();
        assert_eq!((first.length, first.flags), (1024, 12));
        assert_eq!(writer.complete(true, false), 0);
        let last = writer.next_chunk();
        assert_eq!((last.offset, last.length, last.flags), (1024, 976, 20));
        assert_eq!(writer.complete(false, true), -1);
        assert_eq!(writer.complete(true, false), 0);
        assert_eq!(writer.mode, COMPLETE);
        assert_eq!(size_of::<BusHead>(), 40);
    }

    type Received = Vec<(i64, Vec<u8>)>;

    /// `core-tests/src/server/message_bus.cpp`'s `DATA_SIZE`.
    const DATA_SIZE: usize = 2_000_000;

    /// `MB_SEND`'s `String pkt(s)`: `s` aligned to 8 (`SW_MEM_ALIGNED_SIZE`), all but
    /// the last byte random, then a NUL.
    fn mb_packet(s: usize, seed: u64) -> Vec<u8> {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut packet: Vec<u8> = (1..aligned(s))
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()[0]
            })
            .collect();
        packet.push(0);
        packet
    }

    /// `swr_v1_bus_write`'s classifier: `catch_pipe_error(errno) == SW_REDUCE_SIZE`.
    fn pipe_reduce() -> bool {
        let mut nobufs = false;
        let code = swoole_rs_reactor::catch_pipe_error(errno(), &mut nobufs);
        code == REDUCE_SIZE
    }

    /// `MessageBus::read` (`dgram == false`) or `read_with_buffer`: one read,
    /// allocating as the native facade's `receive` does.
    fn mb_read(bus: &mut Bus, fd: RawFd, dgram: bool) -> isize {
        let mut state = BusRead {
            received: 0,
            size: 0,
            data: ptr::null_mut(),
            allocator: ptr::null(),
            chunks: 0,
            resume: false,
            event: 0,
        };
        let mut nobufs = false;
        loop {
            let n = unsafe { bus.read(fd, dgram, &mut state, &mut nobufs) };
            if state.event != BUS_ALLOCATE {
                return n;
            }
            let data = unsafe { libc::realloc(state.data, state.size) };
            unsafe { bus.adopt(data, state.size, (&raw const ALLOCATOR.0).cast()) };
        }
    }

    /// The reading end: the reader's bus and what it has read (session, data).
    struct Loopback {
        fds: [RawFd; 2],
        dgram: bool,
        bus: Bus,
        q: Received,
    }

    /// The writer's send: one blocking writev, then (when it succeeded, so errno stays
    /// the failure's for `pipe_reduce`) every packet the reader can read now. Upstream's
    /// reactor interleaves its writes and reads; reading only whole writes keeps a
    /// partly written stream chunk from being read as a whole one.
    fn loopback_send(pipe: &mut Loopback, iov: &[libc::iovec]) -> isize {
        let count = c_int::try_from(iov.len()).unwrap();
        let sent = unsafe { libc::writev(pipe.fds[0], iov.as_ptr(), count) };
        if sent < 0 {
            return sent;
        }
        loop {
            let mut poll = libc::pollfd {
                fd: pipe.fds[1],
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&raw mut poll, 1, 0) } != 1 {
                return sent;
            }
            let n = mb_read(&mut pipe.bus, pipe.fds[1], pipe.dgram);
            assert!(n >= 0);
            if n > 0 {
                let packet = pipe.bus.packet();
                let data = if packet.length == 0 {
                    Vec::new()
                } else {
                    unsafe { std::slice::from_raw_parts(packet.data.cast::<u8>(), packet.length) }
                        .to_vec()
                };
                pipe.q.push((pipe.bus.head().fd, data));
            }
        }
    }

    /// Both `message_bus` TESTs: a socket pair with 64KiB buffers and a bus of 64KiB;
    /// packets of `DATA_SIZE`, the bus's buffer size and 2341 bytes to sessions 1-3, then
    /// `send_empty_packet`'s two (`hello world` with length 0 to session 4, null with
    /// length 10 to session 5), read until five arrive. Packets are never popped, as
    /// upstream's `TestMB::read` never pops them. The reader's bus, the packets it read
    /// and the three packets sent.
    fn message_bus(kind: c_int, dgram: bool) -> (Bus, Received, Vec<Vec<u8>>) {
        let mut fds = [0; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, kind, 0, fds.as_mut_ptr()) },
            0
        );
        for fd in fds {
            let buffer: c_int = 65536;
            for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
                unsafe {
                    libc::setsockopt(
                        fd,
                        libc::SOL_SOCKET,
                        option,
                        ptr::from_ref(&buffer).cast(),
                        4,
                    )
                };
            }
        }
        let flags = unsafe { libc::fcntl(fds[1], libc::F_GETFL) };
        unsafe { libc::fcntl(fds[1], libc::F_SETFL, flags | libc::O_NONBLOCK) };
        let mut pipe = Loopback {
            fds,
            dgram,
            bus: Bus::new(65536),
            q: Vec::new(),
        };
        let mut buffer = vec![0u64; 65536 / 8];
        pipe.bus.buffer = buffer.as_mut_ptr().cast();
        let sent = vec![
            mb_packet(DATA_SIZE, 1),
            mb_packet(pipe.bus.buffer_size, 2),
            mb_packet(2341, 3),
        ];
        let writer = Bus::new(65536);
        let mut msg_id = 0;
        let mut write = |fd: i64, data: *const u8, len: usize| {
            let mut head = BusHead {
                fd,
                msg_id,
                len: u32::try_from(len).unwrap(),
                ..BusHead::default()
            };
            msg_id += 1;
            let send = |iov: &[libc::iovec]| loopback_send(&mut pipe, iov);
            unsafe { writer.write(&mut head, data, send, pipe_reduce) }
        };
        for (fd, packet) in (1..).zip(&sent) {
            assert!(write(fd, packet.as_ptr(), packet.len()));
        }
        assert!(write(4, b"hello world".as_ptr(), 0));
        assert!(write(5, ptr::null(), 10));
        assert_eq!(pipe.q.len(), 5);
        for fd in fds {
            unsafe { libc::close(fd) };
        }
        pipe.bus.buffer = ptr::null_mut();
        drop(buffer);
        (pipe.bus, pipe.q, sent)
    }

    /// `MB_ASSERT(1..3)` and the empty packets' asserts. `ASSERT_STREQ` compares up to
    /// the first NUL; the whole packet is compared here.
    fn assert_packets(q: &Received, sent: &[Vec<u8>]) {
        for ((fd, packet), (got_fd, got)) in (1..).zip(sent).zip(q) {
            assert_eq!(*got_fd, fd);
            assert!(got == packet, "packet {fd}");
        }
        assert_eq!((q[3].0, q[3].1.as_slice()), (4, &b""[..]));
        assert_eq!((q[4].0, q[4].1.as_slice()), (5, &b""[..]));
    }

    /// `TEST(message_bus, read)`: a stream pair, `MessageBus::read`.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no socket options on a socketpair")]
    fn message_bus_read() {
        let (_bus, q, sent) = message_bus(libc::SOCK_STREAM, false);
        assert_packets(&q, &sent);
    }

    /// `TEST(message_bus, read_with_buffer)`: a datagram pair, `read_with_buffer`. Linux, as
    /// upstream's core-tests: the 64KiB chunks need ENOBUFS to ask for smaller ones, and
    /// only Linux's `catch_pipe_error` reads ENOBUFS that way (macOS caps datagrams at 2KiB).
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no datagram socketpair")]
    #[cfg(target_os = "linux")]
    fn message_bus_read_with_buffer() {
        let (bus, q, sent) = message_bus(libc::SOCK_DGRAM, true);
        assert_packets(&q, &sent);
        assert!(bus.memory_size() > 1024 * 1024 * 2);
    }
}
