//! `MessageBus`, the pipe protocol between reactors, workers and task workers.
//! Rust owns the receive buffer's bookkeeping, the chunked packets being
//! reassembled, and the read and write loops over the pipe's file descriptor.
//!
//! It also owns the bus's own records of its pipes (`init_pipe_socket`), and logs
//! what a read reports.
//!
//! Native code keeps allocation: the allocator may be Zend's, whose `emalloc` can
//! bail out (longjmp), so `swr_v1_bus_read` returns to its caller to allocate a
//! packet and is called again to resume. Freeing a packet calls its allocator's
//! `free`, which never bails out.
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void};
use std::hash::{BuildHasherDefault, Hasher};
use std::ptr;
use swoole_rs_corebase::log;
use swoole_rs_reactor::SocketRecord;

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

/// `BusRead::event`: what the caller does after `swr_v1_bus_read` returns.
pub const BUS_ALLOCATE: u8 = 1;
pub const BUS_CLOSED: u8 = 2;
pub const BUS_ABNORMAL: u8 = 3;
pub const BUS_READV_CLOSED: u8 = 4;

/// `swoole::DataHead`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BusHead {
    pub fd: i64,
    pub msg_id: u64,
    pub len: u32,
    pub reactor_id: i16,
    pub kind: u8,
    pub flags: u8,
    pub server_fd: u16,
    pub ext_flags: u16,
    pub reserved: u32,
    pub time: f64,
}

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

/// Writes `count` iovecs to `socket`; never bails out.
pub type BusSend =
    unsafe extern "C" fn(socket: *mut c_void, iov: *const libc::iovec, count: usize) -> isize;
/// The next message id (`MessageBus`'s `id_generator_`).
pub type BusNextId = unsafe extern "C" fn(context: *mut c_void) -> u64;
/// Whether the failed write's errno asks for smaller chunks (`SW_REDUCE_SIZE`).
pub type BusReduce = unsafe extern "C" fn(socket: *mut c_void) -> bool;

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
    /// `id_generator_`: what numbers each written message.
    next_id: Option<(BusNextId, *mut c_void)>,
}

impl Drop for Bus {
    /// `~MessageBus`: the pipe records go, their descriptors stay open.
    fn drop(&mut self) {
        for &socket in self.pipe_sockets.iter().filter(|socket| !socket.is_null()) {
            // SAFETY: a record `init_pipe_socket` made, used by nothing else now.
            unsafe { (*socket).fd = -1 };
            // SAFETY: as above.
            unsafe { free_socket(socket) };
        }
    }
}

// Outside the hybrid (and in tests, which link no C++) a record has no timers,
// buffers or loop: `Socket::free` is the record's deallocation.
#[cfg(not(all(feature = "upstream-symbols", not(test))))]
use swoole_rs_reactor::swr_v1_socket_dealloc as free_socket;
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
    let fd = unsafe { (*socket).fd };
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
///
/// # Safety
/// `nobufs` is writable.
unsafe fn waits(nobufs: *mut bool) -> bool {
    // SAFETY: the caller lends a writable bool.
    unsafe { swoole_rs_reactor::swr_v1_socket_catch_error(errno(), nobufs) == WAIT }
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

    fn memory_size(&self) -> usize {
        self.buffer_size + self.packets.values().map(|p| p.size).sum::<usize>()
    }

    fn packet(&self) -> BusPacket {
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

    fn pass(&self, info: &BusHead, data: *const c_char) {
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

    fn move_packet(&mut self) -> *mut c_char {
        let id = self.head().msg_id;
        self.packets.get_mut(&id).map_or(ptr::null_mut(), |p| {
            std::mem::replace(&mut p.data, ptr::null_mut()).cast()
        })
    }

    fn pop(&mut self) {
        let head = self.head();
        if head.flags & END != 0 {
            self.packets.remove(&head.msg_id);
        }
    }

    fn adopt(&mut self, data: *mut u8, size: usize, allocator: *const Allocator) {
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
    /// The buffer is set; `nobufs` is writable.
    unsafe fn read(
        &mut self,
        fd: c_int,
        dgram: bool,
        state: &mut BusRead,
        nobufs: *mut bool,
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
                    // SAFETY: the caller lends `nobufs`.
                    return if unsafe { waits(nobufs) } { 0 } else { -1 };
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
                // SAFETY: the caller lends `nobufs`.
                if n < 0 && unsafe { waits(nobufs) } {
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
    /// # Safety
    /// `head` is writable; `data` covers `head.len` bytes or is null; `send` and
    /// `reduce` are valid for `socket`.
    unsafe fn write(
        &self,
        socket: *mut c_void,
        head: &mut BusHead,
        data: *const u8,
        send: BusSend,
        reduce: BusReduce,
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
            // SAFETY: the caller's send for `socket`, over the header and its payload.
            let sent = unsafe { send(socket, iov.as_ptr(), count) };
            // Chunks accept any nonnegative write; direct writes need every byte.
            let success = if chunk.mode == CHUNKS {
                sent >= 0
            } else {
                sent.cast_unsigned() == HEAD + chunk.length as usize
            };
            // SAFETY: the caller's classifier for `socket`, right after its send.
            let smaller = !success && chunk.mode != EMPTY && unsafe { reduce(socket) };
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

/// # Safety
/// `bus` is a live handle from `swr_v1_bus_create`, not used elsewhere for this call.
unsafe fn bus<'a>(bus: *mut Bus) -> &'a mut Bus {
    // SAFETY: the caller's contract.
    unsafe { &mut *bus }
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_bus_create(buffer_size: usize) -> *mut Bus {
    Box::into_raw(Box::new(Bus::new(buffer_size)))
}

/// Frees the bus and its packets (not the buffer: see `swr_v1_bus_set_buffer`).
///
/// # Safety
/// `bus` came from `swr_v1_bus_create` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_free(bus: *mut Bus) {
    // SAFETY: the caller's contract.
    drop(unsafe { Box::from_raw(bus) });
}

/// # Safety
/// As `bus`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_set_buffer_size(bus: *mut Bus, size: usize) {
    // SAFETY: the caller's contract.
    unsafe { self::bus(bus) }.buffer_size = size;
}

/// # Safety
/// As `bus`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_buffer_size(bus: *mut Bus) -> usize {
    // SAFETY: the caller's contract.
    unsafe { self::bus(bus) }.buffer_size
}

/// # Safety
/// As `bus`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_set_always_chunked(bus: *mut Bus) {
    // SAFETY: the caller's contract.
    unsafe { self::bus(bus) }.always_chunked = true;
}

/// Sets the receive buffer, `buffer_size` bytes the caller allocated and frees,
/// and zeroes its header; or forgets it (null).
///
/// # Safety
/// As `bus`; a non-null `buffer` holds `buffer_size` writable bytes and stays
/// valid until it is replaced.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_set_buffer(bus: *mut Bus, buffer: *mut c_void) {
    // SAFETY: the caller's contract.
    let bus = unsafe { self::bus(bus) };
    bus.buffer = buffer.cast();
    if !buffer.is_null() {
        // SAFETY: the caller lends a buffer that starts with a writable header.
        unsafe { bus.buffer.write(BusHead::default()) };
    }
}

/// # Safety
/// As `bus`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_buffer(bus: *mut Bus) -> *mut c_void {
    // SAFETY: the caller's contract.
    unsafe { self::bus(bus) }.buffer.cast()
}

/// Packets being reassembled.
///
/// # Safety
/// As `bus`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_count(bus: *mut Bus) -> usize {
    // SAFETY: the caller's contract.
    unsafe { self::bus(bus) }.packets.len()
}

/// # Safety
/// As `bus`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_clear(bus: *mut Bus) {
    // SAFETY: the caller's contract.
    unsafe { self::bus(bus) }.packets.clear();
}

/// # Safety
/// As `bus`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_memory_size(bus: *mut Bus) -> usize {
    // SAFETY: the caller's contract.
    unsafe { self::bus(bus) }.memory_size()
}

/// `MessageBus::pass`: the buffer points at `data` instead of holding it.
///
/// # Safety
/// As `bus`, with the buffer set; `info` is readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_pass(bus: *mut Bus, info: *const BusHead, data: *const c_char) {
    // SAFETY: the caller's contract.
    let bus = unsafe { self::bus(bus) };
    // SAFETY: the caller lends a readable header.
    bus.pass(unsafe { &*info }, data);
}

/// `MessageBus::get_packet`.
///
/// # Safety
/// As `bus`, with the buffer set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_packet(bus: *mut Bus) -> BusPacket {
    // SAFETY: the caller's contract.
    unsafe { self::bus(bus) }.packet()
}

/// `MessageBus::move_packet`: the current packet's block, now the caller's.
///
/// # Safety
/// As `bus`, with the buffer set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_move_packet(bus: *mut Bus) -> *mut c_char {
    // SAFETY: the caller's contract.
    unsafe { self::bus(bus) }.move_packet()
}

/// `MessageBus::pop`: frees the current packet once its last chunk arrived.
///
/// # Safety
/// As `bus`, with the buffer set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_pop(bus: *mut Bus) {
    // SAFETY: the caller's contract.
    unsafe { self::bus(bus) }.pop();
}

/// Reads from the pipe `fd`; see `Bus::read`. On `BUS_ALLOCATE` the caller
/// allocates (or reallocates) `state.size` bytes, calls `swr_v1_bus_adopt`, and
/// calls this again with the same `state`.
///
/// # Safety
/// As `bus`, with the buffer set; `nobufs` and `state` are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_read(
    bus: *mut Bus,
    fd: c_int,
    dgram: bool,
    nobufs: *mut bool,
    state: *mut BusRead,
) -> isize {
    // SAFETY: the caller's contract.
    let bus = unsafe { self::bus(bus) };
    // SAFETY: the caller lends a writable state.
    let state = unsafe { &mut *state };
    // SAFETY: the caller's contract.
    let n = unsafe { bus.read(fd, dgram, state, nobufs) };
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
                bus.head().reactor_id
            ),
        ),
        BUS_ABNORMAL => log::error_log(
            /* SW_LOG_WARNING */ 4,
            /* SW_ERROR_SERVER_WORKER_ABNORMAL_PIPE_DATA */ 9102,
            origin,
            format_args!(
                "abnormal pipeline data, msg_id={}, pipe_fd={fd}, reactor_id={}",
                bus.head().msg_id,
                bus.head().reactor_id
            ),
        ),
        _ => {}
    }
    n
}

/// Hands the block `BUS_ALLOCATE` asked for to the current packet.
///
/// # Safety
/// As `bus`, with the buffer set; `data` holds `size` bytes from `allocator`
/// (a `swoole::Allocator` that outlives the bus).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_adopt(
    bus: *mut Bus,
    data: *mut c_void,
    size: usize,
    allocator: *const c_void,
) {
    // SAFETY: the caller's contract.
    unsafe { self::bus(bus) }.adopt(data.cast(), size, allocator.cast());
}

/// `MessageBus::set_id_generator`: `next(context)` numbers each message written.
///
/// # Safety
/// As `bus`; `next` may be called with `context` while the bus lives.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_set_id_source(
    bus: *mut Bus,
    next: BusNextId,
    context: *mut c_void,
) {
    // SAFETY: the caller's contract.
    unsafe { self::bus(bus) }.next_id = Some((next, context));
}

/// `MessageBus::write`: numbers the message, then writes it through the event loop
/// while one is available, else synchronously. False if the pipe failed.
///
/// # Safety
/// As `bus`; `socket` is a live record; `head` is writable; `data` covers
/// `head.len` bytes or is null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_write(
    bus: *mut Bus,
    socket: *mut SocketRecord,
    head: *mut BusHead,
    data: *const c_char,
) -> bool {
    /// # Safety
    /// `socket` is a live record and `iov` denotes `count` readable iovecs.
    unsafe extern "C" fn send(socket: *mut c_void, iov: *const libc::iovec, count: usize) -> isize {
        use swoole_rs_reactor::event_api;
        if event_api::swoole_event_is_available() {
            // SAFETY: the write's live record and iovecs, on the thread's live loop.
            unsafe { event_api::swoole_event_writev(socket.cast(), iov, count) }
        } else {
            // SAFETY: the write's live record and iovecs.
            unsafe { writev_sync(socket.cast(), iov, count) }
        }
    }
    /// # Safety
    /// `socket` is a live record.
    unsafe extern "C" fn reduce(socket: *mut c_void) -> bool {
        // SAFETY: the write's live record.
        let nobufs = unsafe { &raw mut (*socket.cast::<SocketRecord>()).kernel_nobufs };
        // SAFETY: `nobufs` is the record's writable field.
        unsafe { swoole_rs_reactor::swr_v1_socket_catch_pipe_error(errno(), nobufs) == REDUCE_SIZE }
    }
    // SAFETY: the caller's contract.
    let bus = unsafe { self::bus(bus) };
    // SAFETY: the caller lends a writable header.
    let head = unsafe { &mut *head };
    if let Some((next, context)) = bus.next_id {
        // SAFETY: the source the caller set for this bus.
        head.msg_id = unsafe { next(context) };
    }
    // SAFETY: the caller's contract.
    unsafe { bus.write(socket.cast(), head, data.cast(), send, reduce) }
}

/// `MessageBus::init_pipe_socket`: this bus's own nonblocking record of the pipe
/// `fd`, with no send-buffer limit.
///
/// # Safety
/// As `bus`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_init_pipe_socket(bus: *mut Bus, fd: c_int) {
    // SAFETY: the caller's contract.
    let bus = unsafe { self::bus(bus) };
    let Ok(index) = usize::try_from(fd) else {
        return;
    };
    if index >= bus.pipe_sockets.len() {
        bus.pipe_sockets.resize(index + 1, ptr::null_mut());
    }
    // SAFETY: `make_socket` returns a new record, which nothing else holds yet.
    let socket = unsafe {
        &mut *swoole_rs_reactor::swr_v1_socket_make(fd, /* SW_FD_PIPE */ 3)
    };
    socket.output.buffer_size = u32::MAX;
    if socket.output.nonblock == 0 {
        // SAFETY: the live record just made.
        unsafe { swoole_rs_reactor::socket_upstream::socket_set_fd_option(socket, 1, -1) };
    }
    bus.pipe_sockets[index] = socket;
}

/// `MessageBus::get_pipe_socket`: the record `init_pipe_socket` made for `fd`.
///
/// # Safety
/// As `bus`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_bus_pipe_socket(bus: *mut Bus, fd: c_int) -> *mut SocketRecord {
    // SAFETY: the caller's contract.
    let bus = unsafe { self::bus(bus) };
    usize::try_from(fd)
        .ok()
        .and_then(|index| bus.pipe_sockets.get(index).copied())
        .unwrap_or(ptr::null_mut())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::RawFd;

    unsafe extern "C" fn free(data: *mut c_void) {
        unsafe { libc::free(data) };
    }
    struct Static(Allocator);
    unsafe impl Sync for Static {}
    static ALLOCATOR: Static = Static(Allocator {
        malloc: ptr::null(),
        calloc: ptr::null(),
        realloc: ptr::null(),
        free,
    });

    unsafe extern "C" fn send(socket: *mut c_void, iov: *const libc::iovec, count: usize) -> isize {
        let fd = RawFd::try_from(socket as usize).unwrap();
        unsafe { libc::writev(fd, iov, c_int::try_from(count).unwrap()) }
    }
    unsafe extern "C" fn never(_: *mut c_void) -> bool {
        false
    }

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
            let socket = self.fds[0].cast_unsigned() as usize as *mut c_void;
            unsafe { self.bus.write(socket, &mut head, data, send, never) }
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
                let n = unsafe {
                    self.bus
                        .read(self.fds[1], dgram, &mut state, &raw mut nobufs)
                };
                if state.event == BUS_ALLOCATE {
                    let data = unsafe { libc::realloc(state.data, state.size) };
                    self.bus
                        .adopt(data.cast(), state.size, &raw const ALLOCATOR.0);
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
            unsafe {
                pipe.bus
                    .read(pipe.fds[1], false, &mut state, &raw mut nobufs)
            },
            0
        );
        assert_eq!((state.event, state.size), (BUS_ALLOCATE, 5000));
        let data = unsafe { libc::malloc(state.size) };
        pipe.bus
            .adopt(data.cast(), state.size, &raw const ALLOCATOR.0);
        assert_eq!(pipe.bus.memory_size(), 4096 + 5000);
        assert!(
            unsafe {
                pipe.bus
                    .read(pipe.fds[1], false, &mut state, &raw mut nobufs)
            } > 0
        );
        assert_eq!(pipe.bus.packet().length, 5000);
        let moved = pipe.bus.move_packet();
        assert_eq!(moved.cast::<c_void>(), data);
        assert!(pipe.bus.packet().data.is_null());
        pipe.bus.pop();
        unsafe { libc::free(moved.cast()) };
    }

    #[test]
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
}
