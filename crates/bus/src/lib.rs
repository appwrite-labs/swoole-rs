//! `MessageBus`, the pipe protocol between reactors, workers and task workers.
//! Rust owns the receive buffer, the chunked packets being
//! reassembled, and the read and write loops over the pipe's file descriptor.
//!
//! It also owns the bus's own records of its pipes (`init_pipe_sockets`), and logs
//! what a read reports.
//!
//! The buffer and each packet's block come from a [`TrustedAllocator`] the bus's
//! owner names. A read returns from `Bus::receive` with nothing borrowed before
//! [`Bus::read_from`] allocates a packet, then resumes. Zend's supported request
//! allocator defers memory-limit errors (docs/zend.md); freeing a block through
//! its allocator's `free` never bails out.
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
#![forbid(unsafe_code)]
use std::cell::{Cell, OnceCell};
use std::collections::HashMap;
use std::ffi::c_int;
use std::hash::{BuildHasherDefault, Hasher};
use std::io::IoSlice;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use swoole_rs_corebase::buffer::string::OwnedString;
use swoole_rs_corebase::constants::{EventDataFlag, FdType, SW_WORKER_MAX_RECV_CHUNK_COUNT};
use swoole_rs_corebase::memory::members::pool::PooledWord;
use swoole_rs_corebase::process::process_pool::RecvData;
use swoole_rs_corebase::reactor::{Caught, Heap, OwnedSocket, Socket, SocketFields, SocketRecord};
use swoole_rs_corebase::sys::{self, errno};
use swoole_rs_corebase::tls::upstream::{socket_free as free_socket, writev_sync};
use swoole_rs_corebase::{BoxCellExt, CellExt};
use swoole_rs_corebase::{DataHead as BusHead, TrustedAllocator, log};

pub use Bus as MessageBus;

/// `SW_EVENT_DATA_*`.
const PTR: u8 = EventDataFlag::SW_EVENT_DATA_PTR.bits();
const CHUNK: u8 = EventDataFlag::SW_EVENT_DATA_CHUNK.bits();
const BEGIN: u8 = EventDataFlag::SW_EVENT_DATA_BEGIN.bits();
const END: u8 = EventDataFlag::SW_EVENT_DATA_END.bits();
const OBJ_PTR: u8 = EventDataFlag::SW_EVENT_DATA_OBJ_PTR.bits();
const HEAD: usize = size_of::<BusHead>();
/// `SW_IPC_MSG_MIN`.
const IPC_MSG_MIN: usize = 2048 - HEAD;
const _: () = assert!(HEAD == 40, "sizeof(DataHead)");
/// Size of the legacy `PacketPtr` (length and data pointer) in a bus message.
const PACKET_PTR_SIZE: u32 = 16;

/// `BusRead::event`: what the caller does after `Bus::receive` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    /// Allocate `BusRead::size` bytes and read again.
    Allocate,
    /// The peer closed the pipe.
    Closed,
    /// A chunk of a packet the bus never began.
    Abnormal,
    /// The peer closed the pipe mid-packet.
    ReadvClosed,
}

/// The current message's bytes, taken out of its bus ([`Bus::take_packet`]) until
/// they go back ([`Bus::restore`]), so nothing the bus does meanwhile frees or
/// overwrites them.
pub struct Packet {
    held: Held,
    /// The message's head when it was taken.
    head: BusHead,
}

enum Held {
    /// A message passed by pointer, whose payload stays with its passer, or none.
    Nothing,
    /// The receive buffer holding a whole message, and the bus's buffer generation
    /// when it was taken.
    Buffer(Box<OwnedString>, usize),
    /// A reassembled packet, out of the bus's table.
    Chunked(OwnedString),
}

impl Packet {
    /// The message's payload: a whole one's `len` bytes after its head (at most the
    /// buffer's), a chunked one's packet, or none.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        match &self.held {
            Held::Nothing => &[],
            Held::Buffer(buffer, _) => {
                let payload = buffer.view().get(HEAD..).unwrap_or_default();
                payload.get(..self.head.len as usize).unwrap_or(payload)
            }
            Held::Chunked(packet) => packet.view(),
        }
    }

    /// The message under the head `info` (its `len` the payload's) for one receive
    /// callback: a chunked packet moves into it, which the callback may take
    /// ([`RecvData::take_packet`]) or leave to be freed with it; a whole message is
    /// lent.
    #[inline]
    pub fn receive(&mut self, info: BusHead) -> RecvData<'_> {
        if let Held::Chunked(packet) = &mut self.held {
            return RecvData::owned(info, std::mem::take(packet));
        }
        RecvData::new(info, self.bytes())
    }
}

/// One read's progress across the allocations it asks for.
struct BusRead {
    /// Buffered reads: the datagram's length, kept across an allocation.
    received: usize,
    /// `Event::Allocate`: the bytes to allocate.
    size: usize,
    chunks: u16,
    /// The buffer holds the current chunk; set by `Event::Allocate`.
    resume: bool,
    event: Option<Event>,
}

impl BusRead {
    /// A read that has seen nothing.
    fn new() -> Self {
        Self {
            received: 0,
            size: 0,
            chunks: 0,
            resume: false,
            event: None,
        }
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

/// The packets being reassembled, by message id.
type Packets = HashMap<u64, OwnedString, BuildHasherDefault<IdHasher>>;

/// This bus's pipe records, by descriptor.
type PipeSockets = Box<[Option<OwnedSocket>]>;

/// Its methods take `&self`: the server lends the bus through a `Copy` handle, so
/// each field is a cell. The packets are boxed, so a take moves a pointer, and are
/// taken out and put back around each access, which asks no [`TrustedAllocator`] and
/// calls nothing back into the server or PHP. The pipe records are set once and
/// freed only by the drop, so a borrow of the bus lends them.
pub struct Bus {
    /// Taken out around reads and callbacks, like the packet table.
    buffer: Cell<Option<Box<OwnedString>>>,
    /// Decoded once after a read; byte writes below keep the wire header in sync.
    current: Cell<BusHead>,
    /// A callback may replace or free the buffer while its old owner is lent.
    buffer_generation: Cell<usize>,
    buffer_size: Cell<usize>,
    always_chunked: Cell<bool>,
    packets: Cell<Option<Box<Packets>>>,
    /// `pipe_sockets_`: by descriptor, this bus's own records of the pipes, which
    /// `init_pipe_sockets` made and only the bus's drop frees.
    pipe_sockets: OnceCell<PipeSockets>,
    /// `id_generator_`: the counter that numbers each written message.
    next_id: Cell<Option<Counter>>,
}

enum Counter {
    Shared(&'static AtomicU64),
    Pooled(PooledWord<AtomicU64>),
}

impl Counter {
    fn next(&self) -> u64 {
        let word = match self {
            Self::Shared(word) => *word,
            Self::Pooled(word) => word,
        };
        word.fetch_add(1, Ordering::SeqCst)
    }
}

impl Drop for Bus {
    /// `~MessageBus`: the pipe records go, their descriptors stay open.
    fn drop(&mut self) {
        // By descriptor, as `init_pipe_sockets` recorded them.
        let records = self.pipe_sockets.take().unwrap_or_default();
        for record in records.into_iter().flatten() {
            record.socket().edit(SocketFields::forget_descriptor);
            free_socket(Some(record));
        }
    }
}

/// `Socket::catch_read_error(errno) == SW_WAIT`.
fn waits(nobufs: &mut bool) -> bool {
    swoole_rs_corebase::reactor::catch_error(errno(), nobufs) == Caught::Wait
}

impl Bus {
    fn new(buffer_size: usize) -> Self {
        Self {
            buffer: Cell::default(),
            current: Cell::default(),
            buffer_generation: Cell::new(0),
            buffer_size: Cell::new(buffer_size),
            always_chunked: Cell::new(false),
            packets: Cell::new(None),
            pipe_sockets: OnceCell::new(),
            next_id: Cell::new(None),
        }
    }

    /// The current message's decoded head; empty before allocation or after freeing.
    #[must_use]
    #[inline]
    pub fn head(&self) -> BusHead {
        self.current.get()
    }

    fn set_head(&self, head: &BusHead) {
        self.buffer.with_box(|buffer| {
            if let Some(to) = buffer.view_mut().get_mut(..HEAD) {
                to.copy_from_slice(&head.to_bytes());
                self.current.set(*head);
            }
        });
    }

    /// Whether the buffer holds the whole current message: its head and then the
    /// `len` bytes it counts, read in one piece (not chunked into a packet, nor
    /// passed by pointer), within the buffer's size.
    #[must_use]
    pub fn holds_whole(&self) -> bool {
        self.with_whole(|_| ()).is_some()
    }

    /// `f` over a whole, initialized message, owned locally through the callback.
    /// The buffer is absent from the bus meanwhile. A callback's replacement/free
    /// takes effect when it returns, without invalidating the bytes it was lent.
    pub fn with_whole<R>(&self, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
        let buffer = self.buffer.take()?;
        let generation = self.buffer_generation.get();
        let result = (|| {
            let head = self.head();
            let length = HEAD.saturating_add(head.len as usize);
            if head.flags & (PTR | CHUNK | OBJ_PTR) != 0 || length > self.buffer_size.get() {
                return None;
            }
            buffer.view().get(..length).map(f)
        })();
        if self.buffer_generation.get() == generation {
            self.buffer.restore_box(buffer);
        }
        result
    }

    fn set_flags(&self, buffer: &mut [u8], flags: u8) {
        if let Some(to) = buffer.get_mut(std::mem::offset_of!(BusHead, flags)) {
            *to = flags;
            self.current.set(BusHead {
                flags,
                ..self.current.get()
            });
        }
    }

    /// Read into the buffer's prefix, retaining bytes beyond a short read.
    fn fill(
        &self,
        buffer: &mut [u8],
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Option<usize>,
    ) -> Option<usize> {
        let received = fill(buffer.get_mut(..length).unwrap_or_default());
        if received.is_some()
            && let Some(bytes) = buffer.first_chunk()
        {
            self.current.set(BusHead::from_bytes(bytes));
        }
        received
    }

    #[must_use]
    pub fn memory_size(&self) -> usize {
        self.buffer_size.get()
            + self
                .packets
                .with_box(|packets| packets.values().map(|p| p.size()).sum::<usize>())
                .unwrap_or(0)
    }

    /// `MessageBus::get_packet`, lent: `f` over the current message's bytes, which
    /// [`Bus::take_packet`] takes out of the bus meanwhile.
    #[inline]
    pub fn with_packet<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        let packet = self.take_packet();
        let result = f(packet.bytes());
        self.restore(packet);
        result
    }

    /// The current message's bytes, owned until [`Bus::restore`] gives them back: a
    /// chunked message's packet, out of the table, or the buffer that holds a whole
    /// one, which the bus lacks meanwhile (a read finds no buffer). A message passed
    /// by pointer (`SW_EVENT_DATA_PTR`), whose payload stays with its passer, holds
    /// none.
    #[inline]
    #[must_use]
    pub fn take_packet(&self) -> Packet {
        let head = self.head();
        let held = if head.flags & OBJ_PTR != 0 {
            self.take_chunked(head.msg_id)
        } else if head.flags & PTR != 0 {
            Held::Nothing
        } else {
            self.buffer.take().map_or(Held::Nothing, |buffer| {
                Held::Buffer(buffer, self.buffer_generation.get())
            })
        };
        Packet { held, head }
    }

    /// Gives back what [`Bus::take_packet`] took: the buffer, unless the bus freed or
    /// set another meanwhile, and a packet the callback left, unless the bus began
    /// its message again; [`Bus::pop`] frees a whole one, as upstream.
    #[inline]
    pub fn restore(&self, packet: Packet) {
        match packet.held {
            Held::Nothing => {}
            Held::Buffer(buffer, generation) => {
                if self.buffer_generation.get() == generation {
                    self.buffer.restore_box(buffer);
                }
            }
            // A packet the callback took leaves an empty one, which needs nothing.
            Held::Chunked(chunked) if !chunked.as_ptr().is_null() => {
                self.restore_chunked(&packet.head, chunked);
            }
            Held::Chunked(_) => {}
        }
    }

    /// The reassembled packet `id`, out of the table; out of line, as only chunked
    /// messages need it.
    #[inline(never)]
    fn take_chunked(&self, id: u64) -> Held {
        self.packets
            .with_box(|packets| packets.remove(&id))
            .flatten()
            .map_or(Held::Nothing, Held::Chunked)
    }

    /// [`Bus::restore`] of a chunked packet.
    #[inline(never)]
    fn restore_chunked(&self, head: &BusHead, packet: OwnedString) {
        self.packets.with_taken(|slot| {
            slot.get_or_insert_with(Box::default)
                .entry(head.msg_id)
                .or_insert(packet);
        });
    }

    /// `MessageBus::pass`'s head: the bus's head becomes `info`, marked as a pointer
    /// to its payload (`SW_EVENT_DATA_PTR`, and the length of a `PacketPtr`) when it
    /// has one, which stays with its caller; nothing while no buffer is set. The
    /// bus's head after.
    pub fn pass(&self, info: &BusHead) -> BusHead {
        if self.buffer.with_box(|_| ()).is_some() {
            let mut head = *info;
            if info.len > 0 {
                head.flags = PTR;
                head.len = PACKET_PTR_SIZE;
            }
            self.set_head(&head);
        }
        self.head()
    }

    /// `MessageBus::pop`: frees the current packet once its last chunk arrived.
    #[inline]
    pub fn pop(&self) {
        let head = self.head();
        if head.flags & END != 0 {
            self.remove(head.msg_id);
        }
    }

    /// Frees the packet `id`, once the packets are back. Out of line, so that the
    /// check in [`Bus::pop`] stays cheap on every message.
    #[inline(never)]
    fn remove(&self, id: u64) {
        drop(self.packets.with_box(|packets| packets.remove(&id)));
    }

    /// Allocates the block `Event::Allocate` asked for and hands it to the current
    /// packet: a new block from `default`, or the packet's own, grown by the
    /// allocator that made it.
    fn grow(&self, state: &BusRead, default: TrustedAllocator) {
        let id = self.head().msg_id;
        let mut packet = self
            .packets
            .with_box(|packets| packets.remove(&id))
            .flatten()
            .unwrap_or_default();
        if packet.as_ptr().is_null() {
            packet.alloc_exact(state.size, Some(default));
        } else {
            packet.reserve_exact(state.size);
        }
        drop(
            self.packets
                .with_taken(|slot| slot.get_or_insert_with(Box::default).insert(id, packet)),
        );
    }

    /// `MessageBus::read` (stream pipes, `dgram == false`) and
    /// `MessageBus::read_with_buffer` (datagram pipes): a whole packet's length,
    /// `Some(0)` to wait (or allocate, see `state.event`), `None` (upstream's -1) on
    /// an error or with no buffer set.
    fn read(
        &self,
        fd: c_int,
        dgram: bool,
        state: &mut BusRead,
        nobufs: &mut bool,
    ) -> Option<usize> {
        state.event = None;
        self.buffer
            .with_box(|buffer| self.read_buffer(buffer.view_mut(), fd, dgram, state, nobufs))
            .flatten()
    }

    fn read_buffer(
        &self,
        buffer: &mut [u8],
        fd: c_int,
        dgram: bool,
        state: &mut BusRead,
        nobufs: &mut bool,
    ) -> Option<usize> {
        let buffer_size = self.buffer_size.get();
        loop {
            if state.resume {
                state.resume = false;
            } else {
                let n = if dgram {
                    self.fill(buffer, buffer_size, |slots| sys::read(fd, slots))
                } else {
                    self.fill(buffer, HEAD.min(buffer_size), |slots| {
                        sys::recv(fd, slots, libc::MSG_PEEK)
                    })
                };
                let Some(n) = n else {
                    return waits(nobufs).then_some(0);
                };
                if n == 0 {
                    state.event = Some(Event::Closed);
                    return None;
                }
                let head = self.head();
                if dgram {
                    state.chunks = state.chunks.wrapping_add(1);
                    state.received = n;
                }
                if head.flags & CHUNK == 0 {
                    if dgram {
                        return Some(n);
                    }
                    return self.fill(
                        buffer,
                        (HEAD + head.len as usize).min(buffer_size),
                        |slots| sys::read(fd, slots),
                    );
                }
            }
            let head = self.head();
            // The packets stay out across the stream's readv: one syscall on a
            // nonblocking pipe, which neither yields nor calls back.
            let read = self
                .packets
                .with_box(|packets| {
                    let packet = packets.get_mut(&head.msg_id)?;
                    Some('chunk: {
                        Ok(if dgram {
                            let count = state.received.saturating_sub(HEAD);
                            let length = packet.len() + count;
                            if length > packet.size() || packet.as_ptr().is_null() {
                                Self::allocate(state, length);
                                break 'chunk Err(Some(0));
                            }
                            let bytes = buffer.get(HEAD..HEAD + count).unwrap_or_default();
                            packet.fill(|spare| {
                                let _ = spare.put(bytes);
                            });
                            Some(state.received)
                        } else {
                            let remain = (head.len as usize).saturating_sub(packet.len());
                            let n = packet.fill_within((buffer_size - HEAD).min(remain), |spare| {
                                self.fill(buffer, HEAD.min(buffer_size), |header| {
                                    spare.readv_after(fd, header)
                                })
                            });
                            if n == Some(0) {
                                state.event = Some(Event::ReadvClosed);
                                break 'chunk Err(None);
                            }
                            if n.is_none() && waits(nobufs) {
                                break 'chunk Err(Some(0));
                            }
                            n
                        })
                    })
                })
                .flatten();
            let n = match read {
                Some(Ok(n)) => n,
                Some(Err(n)) => return n,
                None if head.flags & BEGIN != 0 => {
                    Self::allocate(state, head.len as usize);
                    return Some(0);
                }
                None => {
                    state.event = Some(Event::Abnormal);
                    if dgram {
                        return None;
                    }
                    // Read the chunk's header from the pipe and discard it.
                    _ = self.fill(buffer, HEAD.min(buffer_size), |slots| {
                        sys::recv(fd, slots, 0)
                    });
                    return Some(0);
                }
            };
            state.chunks = state.chunks.wrapping_add(1);
            let flags = self.head().flags;
            if flags & END != 0 {
                self.set_flags(buffer, flags | OBJ_PTR);
                return n;
            }
            if u32::from(state.chunks) >= SW_WORKER_MAX_RECV_CHUNK_COUNT {
                return Some(0);
            }
        }
    }

    /// Ask the caller to grow the current packet to `size` bytes.
    fn allocate(state: &mut BusRead, size: usize) {
        state.event = Some(Event::Allocate);
        state.resume = true;
        state.size = size;
    }

    /// `MessageBus::write`: one direct write, or chunks of at most the buffer's
    /// payload, halved down to `SW_IPC_MSG_MIN` while the pipe asks for less.
    ///
    /// `send` writes the vectors to the pipe and never bails out; `reduce` says
    /// whether the failed write's errno asks for smaller chunks (`SW_REDUCE_SIZE`).
    /// Each call's vectors are `head`'s bytes and, with `data`, a chunk of its first
    /// `head.len` bytes (none past its end).
    fn write(
        &self,
        head: &mut BusHead,
        data: Option<&[u8]>,
        mut send: impl FnMut(&[IoSlice<'_>]) -> Option<usize>,
        mut reduce: impl FnMut() -> bool,
    ) -> bool {
        let maximum = u32::try_from(self.buffer_size.get() - HEAD).unwrap_or(u32::MAX);
        let Some(mut writer) = BusWriter::new(
            head.len,
            maximum,
            u32::try_from(IPC_MSG_MIN).unwrap_or(u32::MAX),
            data.is_some(),
            self.always_chunked.get(),
        ) else {
            return false;
        };
        while writer.mode != Mode::Complete {
            let chunk = writer.next_chunk();
            head.flags = chunk.flags;
            if chunk.mode == Mode::Empty {
                head.len = 0;
            }
            let bytes = head.to_bytes();
            let start = chunk.offset as usize;
            let payload = data
                .and_then(|data| data.get(start..start.wrapping_add(chunk.length as usize)))
                .unwrap_or_default();
            let iov = [IoSlice::new(&bytes), IoSlice::new(payload)];
            let count = if chunk.mode == Mode::Empty { 1 } else { 2 };
            let sent = send(iov.get(..count).unwrap_or_default());
            // Chunks accept any nonnegative write; direct writes need every byte.
            let success = if chunk.mode == Mode::Chunks {
                sent.is_some()
            } else {
                sent == Some(HEAD + chunk.length as usize)
            };
            let smaller = !success && chunk.mode != Mode::Empty && reduce();
            if !writer.complete(success, smaller) {
                return false;
            }
            head.flags = writer.flags;
        }
        true
    }
}

/// How a [`BusWriter`] sends what is left: only the head (no payload), the payload
/// in one write, in chunks of at most its maximum, or nothing (done).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Empty,
    Direct,
    Chunks,
    Complete,
}

#[derive(Clone, Copy, Debug, Default)]
struct BusWriter {
    remaining: u32,
    offset: u32,
    maximum: u32,
    minimum: u32,
    flags: u8,
    mode: Mode,
}
#[derive(Clone, Copy, Debug, Default)]
struct BusChunk {
    offset: u32,
    length: u32,
    flags: u8,
    mode: Mode,
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
            writer.mode = Mode::Direct;
        } else {
            writer.mode = Mode::Chunks;
            writer.flags = CHUNK | BEGIN;
        }
        Some(writer)
    }

    fn next_chunk(&self) -> BusChunk {
        let length = if self.mode == Mode::Chunks {
            self.remaining.min(self.maximum)
        } else {
            self.remaining
        };
        BusChunk {
            offset: self.offset,
            length,
            flags: self.flags
                | if self.mode == Mode::Chunks && self.remaining <= self.maximum {
                    END
                } else {
                    0
                },
            mode: self.mode,
        }
    }

    /// Records the current chunk's write: on success, moves past it; on failure, when
    /// `reduce` freed the way, retries it in chunks of the minimum. False when the
    /// write failed and cannot be retried smaller.
    fn complete(&mut self, success: bool, reduce: bool) -> bool {
        if !success {
            if !reduce || self.maximum <= self.minimum {
                return false;
            }
            self.maximum = self.minimum;
            if self.mode == Mode::Direct {
                self.mode = Mode::Chunks;
                self.flags = CHUNK | BEGIN;
            } else {
                self.flags &= !END;
            }
            return true;
        }
        let chunk = self.next_chunk();
        self.flags = chunk.flags & !BEGIN;
        if self.mode == Mode::Chunks {
            self.remaining -= chunk.length;
            self.offset += chunk.length;
        } else {
            self.remaining = 0;
        }
        if self.remaining == 0 {
            self.mode = Mode::Complete;
        }
        true
    }
}

/// `MessageBus`'s operations for the server and pools that hold one.
impl Bus {
    /// `new MessageBus()` with no buffer yet.
    #[must_use]
    pub fn create(buffer_size: usize) -> Box<Self> {
        Box::new(Self::new(buffer_size))
    }

    #[must_use]
    pub fn buffer_size(&self) -> usize {
        self.buffer_size.get()
    }

    pub fn set_always_chunked(&self) {
        self.always_chunked.set(true);
    }

    /// `MessageBus::alloc_buffer()`: replace the receive buffer with `size` bytes
    /// from `allocator`, initialized once for safe reads. False, leaving no buffer,
    /// on failure.
    pub fn alloc_buffer(&self, size: usize, allocator: TrustedAllocator) -> bool {
        let block = size.max(HEAD + PACKET_PTR_SIZE as usize);
        let mut buffer = OwnedString::try_alloc_exact(block, allocator);
        if let Some(buffer) = buffer.as_mut() {
            buffer.read_zeroed(block, |bytes| bytes.len());
        }
        let allocated = buffer.is_some();
        self.buffer_size.set(size);
        self.free_buffer();
        self.buffer.set(buffer.map(Box::new));
        allocated
    }

    /// Return the receive allocation to its own allocator, leaving no buffer.
    pub fn free_buffer(&self) {
        self.buffer_generation
            .set(self.buffer_generation.get().wrapping_add(1));
        self.current.set(BusHead::default());
        drop(self.buffer.take());
    }

    /// Packets being reassembled.
    #[must_use]
    pub fn count(&self) -> usize {
        self.packets.with_box(|packets| packets.len()).unwrap_or(0)
    }

    /// Frees every packet, once the (empty) packets are back.
    pub fn clear(&self) {
        drop(self.packets.with_box(std::mem::take));
    }

    /// `MessageBus::read(sock)` (`read_with_buffer` when `dgram`): allocates each
    /// packet block the read asks for, a new one from `allocator`. A whole packet's
    /// length, `Some(0)` to wait, `None` (upstream's -1) on an error.
    pub fn read_from(
        &self,
        allocator: TrustedAllocator,
        sock: Socket<'_>,
        dgram: bool,
    ) -> Option<usize> {
        let mut state = BusRead::new();
        loop {
            // The receive only ever sets `kernel_nobufs`, so it edits the record only then:
            // in thread mode, other threads read this worker's pipe meanwhile.
            let mut nobufs = false;
            let n = self.receive(sock.fd(), dgram, &mut nobufs, &mut state);
            if nobufs {
                sock.edit(|sock| sock.kernel_nobufs = true);
            }
            if state.event != Some(Event::Allocate) {
                return n;
            }
            self.grow(&state, allocator);
        }
    }

    /// Reads from the pipe `fd`; see `Bus::read`. On `Event::Allocate` the caller
    /// allocates `state.size` bytes ([`Bus::grow`]) and calls this again with the
    /// same `state`.
    fn receive(
        &self,
        fd: c_int,
        dgram: bool,
        nobufs: &mut bool,
        state: &mut BusRead,
    ) -> Option<usize> {
        let n = self.read(fd, dgram, state, nobufs);
        let origin = if dgram {
            c"MessageBus::read_with_buffer"
        } else {
            c"MessageBus::read"
        };
        match state.event {
            Some(Event::Closed) => log::warning(
                origin,
                format_args!("receive data from socket#{fd} returns 0"),
            ),
            Some(Event::ReadvClosed) => log::warning(
                origin,
                format_args!(
                    "receive pipeline data error, pipe_fd={fd}, reactor_id={}",
                    self.head().reactor_id
                ),
            ),
            Some(Event::Abnormal) => log::error_log(
                swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING,
                swoole_rs_error::ErrorCode::SW_ERROR_SERVER_WORKER_ABNORMAL_PIPE_DATA as c_int,
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
    pub fn count_ids(&self, counter: &'static AtomicU64) {
        self.next_id.set(Some(Counter::Shared(counter)));
    }

    /// Start numbering from one with a counter shared by processes forked from
    /// this bus. Dropping or replacing the counter releases this process's pin.
    #[must_use]
    pub fn count_pool_ids(&self) -> Option<()> {
        let counter = PooledWord::<AtomicU64>::allocate()?;
        counter.store(1, Ordering::Relaxed);
        self.next_id.set(Some(Counter::Pooled(counter)));
        Some(())
    }

    /// `MessageBus::write` of a message its caller holds as a `SendData`: a head and,
    /// unless it has none, the payload its `len` counts (at most `data`'s bytes are
    /// sent). Numbers the message, then writes it through the event loop while one is
    /// available, else synchronously. False if the pipe failed. (What the event loop
    /// cannot send at once it queues, and watches `socket` until it drains: a [`Heap`]
    /// socket may be.)
    pub fn send(&self, socket: Heap<'_>, head: &mut BusHead, data: Option<&[u8]>) -> bool {
        use swoole_rs_corebase::reactor::event_api;
        let send = |vectors: &[IoSlice<'_>]| {
            if event_api::swoole_event_is_available() {
                event_api::swoole_event_writev(socket, vectors)
            } else {
                writev_sync(*socket, vectors)
            }
        };
        let reduce = || {
            let error = errno();
            socket.catch_pipe_error(error) == Caught::ReduceSize
        };
        if let Some(id) = self
            .next_id
            .with_taken(|counter| counter.as_ref().map(Counter::next))
        {
            head.msg_id = id;
        }
        self.write(head, data, send, reduce)
    }

    /// `MessageBus::init_pipe_socket` of each of `fds`: this bus's own nonblocking
    /// records of the pipes, with no send-buffer limit. A thread that starts again
    /// over its bus (a reload) starts each record over, as upstream's new one would.
    pub fn init_pipe_sockets(&self, fds: impl IntoIterator<Item = c_int>) {
        // `make_socket`: a new record, which the bus frees on drop.
        let make = |fd| {
            let mut socket = SocketRecord::with_fd(fd, FdType::SW_FD_PIPE as c_int);
            socket.limit_output(u32::MAX);
            swoole_rs_corebase::reactor::socket_upstream::socket_make_nonblocking(&mut socket);
            socket
        };
        let fds = fds
            .into_iter()
            .filter_map(|fd| Some((usize::try_from(fd).ok()?, fd)));
        if let Some(sockets) = self.pipe_sockets.get() {
            for (index, fd) in fds {
                let Some(socket) = sockets.get(index).and_then(Option::as_ref) else {
                    swoole_rs_corebase::fatal("a message bus's pipes changed");
                };
                socket.socket().edit(|fields| *fields = make(fd).fields);
            }
            return;
        }
        let mut sockets = Vec::new();
        for (index, fd) in fds {
            if index >= sockets.len() {
                sockets.resize_with(index + 1, || None);
            }
            if let Some(slot) = sockets.get_mut(index) {
                *slot = Some(OwnedSocket::new(Box::new(make(fd))));
            }
        }
        _ = self.pipe_sockets.set(sockets.into_boxed_slice());
    }

    /// `MessageBus::get_pipe_socket`: the record `init_pipe_socket` made for `fd`.
    #[must_use]
    #[inline]
    pub fn pipe_socket(&self, fd: c_int) -> *mut SocketRecord {
        self.pipe(fd)
            .map_or(ptr::null_mut(), |socket| socket.as_ptr())
    }

    /// [`MessageBus::pipe_socket`]'s record as a handle, or `None` before
    /// `init_pipe_sockets` made one for `fd`.
    #[must_use]
    #[inline]
    pub fn pipe(&self, fd: c_int) -> Option<Heap<'_>> {
        let index = usize::try_from(fd).ok()?;
        self.pipe_sockets
            .get()?
            .get(index)?
            .as_ref()
            .map(OwnedSocket::heap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::RawFd;

    use std::ffi::c_void;

    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/run_in_child.rs"
    ));

    #[test]
    fn a_bus_without_a_buffer_reads_nothing_and_has_no_packet() {
        let bus = Bus::new(64);
        let mut state = BusRead::new();
        let mut nobufs = false;
        assert_eq!(bus.receive(-1, false, &mut nobufs, &mut state), None);
        assert_eq!(state.event, None, "no event");
        bus.pass(&BusHead::default());
        assert_eq!(bus.with_packet(<[u8]>::len), 0);
        let mut packet = bus.take_packet();
        assert!(packet.receive(BusHead::default()).take_packet().is_none());
        bus.restore(packet);
        bus.pop();
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no socket options on a socketpair")]
    fn a_second_init_starts_the_same_pipe_records_over() {
        let pipe = Pipe::new(libc::SOCK_STREAM, 64, false);
        let fd = pipe.fds[1];
        pipe.bus.init_pipe_sockets([fd]);
        let record = pipe.bus.pipe_socket(fd);
        pipe.bus.pipe(fd).unwrap().edit(|f| f.limit_output(1));
        pipe.bus.init_pipe_sockets([fd]);
        assert_eq!(pipe.bus.pipe_socket(fd), record, "the same record");
        let size = pipe
            .bus
            .pipe(fd)
            .unwrap()
            .read(swoole_rs_corebase::reactor::SocketFields::output_limit);
        assert_eq!(size, u32::MAX, "started over");
        assert!(pipe.bus.pipe(pipe.fds[0]).is_none());
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no socket options on a socketpair")]
    fn send_numbers_each_message_from_the_shared_counter() {
        use swoole_rs_corebase::memory::members::pool;

        static COUNTER: AtomicU64 = AtomicU64::new(10);
        if run_in_child("tests::send_numbers_each_message_from_the_shared_counter") {
            return;
        }
        swoole_rs_corebase::swr_v1_core_set_pagesize(4096);
        pool::init();
        let mut pipe = Pipe::new(libc::SOCK_STREAM, 64, false);
        pipe.bus.count_pool_ids().unwrap();
        let socket = OwnedSocket::new(Box::new(SocketRecord::with_fd(
            pipe.fds[0],
            FdType::SW_FD_PIPE as c_int,
        )));
        let pid = swoole_rs_corebase::fork_exec::<()>(|| {
            let mut head = BusHead {
                len: 3,
                fd: 7,
                ..BusHead::default()
            };
            if !pipe.bus.send(socket.heap(), &mut head, Some(b"one")) || head.msg_id != 1 {
                std::process::exit(1);
            }
            Ok(())
        })
        .unwrap();
        assert!(pid > 0);
        let mut status = 0;
        assert_eq!(sys::waitpid(pid, Some(&mut status), 0).unwrap(), Some(pid));
        assert_eq!(status, 0);
        assert_eq!(pipe.read(false).as_deref(), Some(&b"one"[..]));
        for (expected, payload) in [(2, &b"two"[..]), (10, &b"ten"[..])] {
            if expected == 10 {
                pipe.bus.count_ids(&COUNTER);
                pool::delete();
            }
            let mut head = BusHead {
                msg_id: 99,
                len: u32::try_from(payload.len()).unwrap(),
                fd: 7,
                ..BusHead::default()
            };
            assert!(pipe.bus.send(socket.heap(), &mut head, Some(payload)));
            assert_eq!(head.msg_id, expected);
            assert_eq!(pipe.read(false).as_deref(), Some(payload));
        }
        assert_eq!(COUNTER.load(Ordering::Relaxed), 11);
        socket.free();
    }

    struct Pipe {
        bus: Bus,
        fds: [RawFd; 2],
    }
    impl Pipe {
        fn new(kind: c_int, size: usize, always: bool) -> Self {
            let fds = sys::socketpair(libc::AF_UNIX, kind, 0).unwrap();
            for fd in fds {
                let flags = sys::fcntl(fd, libc::F_GETFL, 0).unwrap();
                sys::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK).unwrap();
                let buffer = c_int::try_from(1 << 20).unwrap();
                for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
                    sys::setsockopt(fd, libc::SOL_SOCKET, option, &buffer).unwrap();
                }
            }
            let bus = Bus::new(size);
            bus.always_chunked.set(always);
            assert!(bus.alloc_buffer(size, TrustedAllocator::STD));
            Self { bus, fds }
        }
        fn write(&self, id: u64, payload: &[u8]) -> bool {
            let mut head = BusHead {
                msg_id: id,
                len: u32::try_from(payload.len()).unwrap(),
                fd: 7,
                ..BusHead::default()
            };
            let data = (!payload.is_empty()).then_some(payload);
            let fd = self.fds[0];
            let send = |iov: &[IoSlice<'_>]| sys::writev(fd, iov);
            self.bus.write(&mut head, data, send, || false)
        }
        /// Reads until a packet completes, allocating like the native facade.
        fn read(&mut self, dgram: bool) -> Option<Vec<u8>> {
            let mut state = BusRead::new();
            let mut nobufs = false;
            loop {
                let n = self.bus.read(self.fds[1], dgram, &mut state, &mut nobufs);
                if state.event == Some(Event::Allocate) {
                    self.bus.grow(&state, TrustedAllocator::STD);
                    continue;
                }
                if n? == 0 {
                    state.chunks = 0;
                    continue;
                }
                let bytes = self.bus.with_packet(<[u8]>::to_vec);
                assert_eq!(self.bus.head().fd, 7);
                self.bus.pop();
                return Some(bytes);
            }
        }
    }
    impl Drop for Pipe {
        fn drop(&mut self) {
            for fd in self.fds {
                sys::close(fd).unwrap();
            }
            assert_ne!(self.bus.buffer.with_box(|buffer| buffer.len()).unwrap(), 0);
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
        assert_eq!(pipe.bus.count(), 0);
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
        assert_eq!(pipe.bus.count(), 0);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no socket options on a socketpair")]
    fn moved_packets_are_not_freed_and_memory_counts_reassembly() {
        let pipe = Pipe::new(libc::SOCK_STREAM, 4096, true);
        let reassemble = |id| {
            assert!(pipe.write(id, &payload(5000)));
            let mut state = BusRead::new();
            let mut nobufs = false;
            assert_eq!(
                pipe.bus.read(pipe.fds[1], false, &mut state, &mut nobufs),
                Some(0)
            );
            assert_eq!((state.event, state.size), (Some(Event::Allocate), 5000));
            pipe.bus.grow(&state, TrustedAllocator::STD);
            let data = pipe
                .bus
                .packets
                .with_box(|packets| packets.get(&id).unwrap().as_ptr())
                .unwrap()
                .cast::<c_void>();
            assert_eq!(pipe.bus.memory_size(), 4096 + 5000);
            assert!(pipe.bus.read(pipe.fds[1], false, &mut state, &mut nobufs) > Some(0));
            data
        };
        let data = reassemble(9);
        let mut packet = pipe.bus.take_packet();
        assert_eq!(pipe.bus.memory_size(), 4096, "out of the bus while taken");
        let mut recv = packet.receive(pipe.bus.head());
        assert_eq!(recv.payload().len(), 5000);
        assert_eq!(recv.payload().as_ptr().cast::<c_void>(), data, "in place");
        let moved = recv.take_packet().unwrap();
        assert_eq!(moved.as_ptr().cast::<c_void>(), data, "moved, not copied");
        assert_eq!(recv.payload(), []);
        drop(recv);
        pipe.bus.restore(packet);
        assert_eq!(pipe.bus.with_packet(<[u8]>::len), 0, "gone from the bus");
        pipe.bus.pop();
        drop(moved);
        // Left in the message, the packet goes with it.
        reassemble(10);
        let mut packet = pipe.bus.take_packet();
        assert_eq!(packet.receive(pipe.bus.head()).payload().len(), 5000);
        pipe.bus.restore(packet);
        assert_eq!(pipe.bus.memory_size(), 4096, "freed with the message");
        pipe.bus.pop();
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
        let sent = sys::send(pipe.fds[0], &head.to_bytes(), 0);
        assert_eq!(sent, Some(HEAD));
        assert_eq!(pipe.read(true), None);
        assert_eq!(pipe.bus.count(), 0);
    }

    #[test]
    fn the_buffer_lends_the_counted_bytes_and_a_pointer_head_none() {
        let bus = Bus::new(128);
        assert!(bus.alloc_buffer(128, TrustedAllocator::STD));
        bus.buffer.with_box(|buffer| {
            buffer.view_mut()[HEAD..HEAD + 5].copy_from_slice(b"hello");
        });
        let head = |len| {
            bus.set_head(&BusHead {
                len,
                ..BusHead::default()
            });
        };
        head(5);
        bus.with_packet(|packet| {
            assert_eq!(packet, b"hello");
            assert!(!bus.holds_whole(), "the buffer is out while lent");
        });
        assert!(bus.holds_whole(), "and back after");
        head(500);
        assert_eq!(
            bus.with_packet(<[u8]>::len),
            128 - HEAD,
            "at most the buffer"
        );
        let info = BusHead {
            len: 5,
            fd: 3,
            ..BusHead::default()
        };
        let passed = bus.pass(&info);
        assert_eq!((passed.flags, passed.len, passed.fd), (PTR, 16, 3));
        assert_eq!(
            (bus.head().flags, bus.head().len),
            (PTR, 16),
            "the bus's head"
        );
        assert_eq!(
            bus.with_packet(<[u8]>::len),
            0,
            "a passed payload stays its caller's"
        );
        assert!(!bus.holds_whole(), "nor is it in the buffer");
    }

    #[test]
    fn a_buffer_freed_while_lent_goes_once_the_lend_ends() {
        let bus = Bus::new(128);
        assert!(bus.alloc_buffer(128, TrustedAllocator::STD));
        bus.set_head(&BusHead {
            len: 4,
            ..BusHead::default()
        });
        bus.with_packet(|packet| {
            bus.free_buffer();
            assert!(bus.alloc_buffer(64, TrustedAllocator::STD));
            assert_eq!(packet.len(), 4, "the lent bytes stay");
        });
        assert_eq!(
            bus.buffer.with_box(|buffer| buffer.len()),
            Some(64),
            "the new one stays"
        );
    }

    #[test]
    fn the_buffer_holds_a_whole_message_only_unchunked_and_within_its_size() {
        assert!(!Bus::new(128).holds_whole(), "no buffer");
        let bus = Bus::new(128);
        assert!(bus.alloc_buffer(128, TrustedAllocator::STD));
        let head = |len, flags| {
            bus.set_head(&BusHead {
                len,
                flags,
                ..BusHead::default()
            });
        };
        head(88, 0);
        assert!(bus.holds_whole(), "a head and 88 bytes fill 128");
        head(89, 0);
        assert!(!bus.holds_whole(), "past the buffer");
        for flags in [CHUNK, CHUNK | END | OBJ_PTR] {
            head(8, flags);
            assert!(!bus.holds_whole(), "chunked into a packet");
        }
    }

    #[test]
    fn reduction_retries_current_chunk_and_preserves_begin_end() {
        let mut writer = BusWriter::new(2000, 4096, 1024, true, false).unwrap();
        assert_eq!(writer.next_chunk().mode, Mode::Direct);
        assert!(writer.complete(false, true));
        let first = writer.next_chunk();
        assert_eq!((first.length, first.flags), (1024, 12));
        assert!(writer.complete(true, false));
        let last = writer.next_chunk();
        assert_eq!((last.offset, last.length, last.flags), (1024, 976, 20));
        assert!(!writer.complete(false, true));
        assert!(writer.complete(true, false));
        assert_eq!(writer.mode, Mode::Complete);
        assert_eq!(size_of::<BusHead>(), 40);
    }

    #[test]
    fn receive_storage_preserves_prefix_and_callback_ownership() {
        let bus = Bus::new(128);
        assert!(bus.alloc_buffer(128, TrustedAllocator::STD));
        let header = BusHead {
            len: 4,
            fd: 7,
            ..BusHead::default()
        }
        .to_bytes();
        let mut message = header.to_vec();
        message.extend_from_slice(b"data");
        assert_eq!(
            bus.buffer
                .with_box(|buffer| bus.fill(buffer.view_mut(), 128, |room| {
                    room[..message.len()].copy_from_slice(&message);
                    Some(message.len())
                }))
                .flatten(),
            Some(message.len())
        );
        assert_eq!(bus.with_whole(<[u8]>::to_vec), Some(message.clone()));
        // A short overwrite updates the decoded head and retains later bytes.
        let mut changed = header;
        changed[0] ^= 1;
        message[0] = changed[0];
        assert_eq!(
            bus.buffer
                .with_box(|buffer| bus.fill(buffer.view_mut(), 1, |room| {
                    room[0] = changed[0];
                    Some(1)
                }))
                .flatten(),
            Some(1)
        );
        assert_eq!(bus.head().fd, BusHead::from_bytes(&changed).fd);
        assert_eq!(
            bus.with_whole(|bytes| {
                bus.free_buffer();
                bytes.to_vec()
            }),
            Some(message)
        );
        assert!(!bus.holds_whole());
        assert_eq!(bus.head().fd, 0);
        assert!(bus.alloc_buffer(128, TrustedAllocator::STD));
        let header = bus.head().to_bytes();
        bus.with_whole(|bytes| {
            assert!(bus.alloc_buffer(64, TrustedAllocator::STD));
            assert_eq!(bytes, header);
        });
        assert!(
            bus.holds_whole(),
            "the callback's replacement remains installed"
        );
        assert_eq!(bus.buffer_size.get(), 64);
    }

    type Received = Vec<(i64, Vec<u8>)>;

    /// `core-tests/src/server/message_bus.cpp`'s `DATA_SIZE`.
    const DATA_SIZE: usize = 2_000_000;

    fn aligned(size: usize) -> usize {
        let alignment = size_of::<std::ffi::c_ulong>();
        size.wrapping_add(alignment - 1) & !(alignment - 1)
    }

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
        let code = swoole_rs_corebase::reactor::catch_pipe_error(errno(), &mut nobufs);
        code == Caught::ReduceSize
    }

    /// `MessageBus::read` (`dgram == false`) or `read_with_buffer`: one read,
    /// allocating as the native facade's `receive` does.
    fn mb_read(bus: &mut Bus, fd: RawFd, dgram: bool) -> Option<usize> {
        let mut state = BusRead::new();
        let mut nobufs = false;
        loop {
            let n = bus.read(fd, dgram, &mut state, &mut nobufs);
            if state.event != Some(Event::Allocate) {
                return n;
            }
            bus.grow(&state, TrustedAllocator::STD);
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
    fn loopback_send(pipe: &mut Loopback, iov: &[IoSlice<'_>]) -> Option<usize> {
        let sent = sys::writev(pipe.fds[0], iov)?;
        loop {
            let mut poll = libc::pollfd {
                fd: pipe.fds[1],
                events: libc::POLLIN,
                revents: 0,
            };
            if sys::poll(std::slice::from_mut(&mut poll), 0).ok() != Some(1) {
                return Some(sent);
            }
            let n = mb_read(&mut pipe.bus, pipe.fds[1], pipe.dgram);
            assert!(n.is_some());
            if n > Some(0) {
                let data = pipe.bus.with_packet(<[u8]>::to_vec);
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
        let fds = sys::socketpair(libc::AF_UNIX, kind, 0).unwrap();
        for fd in fds {
            let buffer: c_int = 65536;
            for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
                sys::setsockopt(fd, libc::SOL_SOCKET, option, &buffer).unwrap();
            }
        }
        let flags = sys::fcntl(fds[1], libc::F_GETFL, 0).unwrap();
        sys::fcntl(fds[1], libc::F_SETFL, flags | libc::O_NONBLOCK).unwrap();
        let mut pipe = Loopback {
            fds,
            dgram,
            bus: Bus::new(65536),
            q: Vec::new(),
        };
        assert!(pipe.bus.alloc_buffer(65536, TrustedAllocator::STD));
        let sent = vec![
            mb_packet(DATA_SIZE, 1),
            mb_packet(pipe.bus.buffer_size.get(), 2),
            mb_packet(2341, 3),
        ];
        let writer = Bus::new(65536);
        let mut msg_id = 0;
        let mut write = |fd: i64, data: Option<&[u8]>, len: usize| {
            let mut head = BusHead {
                fd,
                msg_id,
                len: u32::try_from(len).unwrap(),
                ..BusHead::default()
            };
            msg_id += 1;
            let send = |iov: &[IoSlice<'_>]| loopback_send(&mut pipe, iov);
            writer.write(&mut head, data, send, pipe_reduce)
        };
        for (fd, packet) in (1..).zip(&sent) {
            assert!(write(fd, Some(packet), packet.len()));
        }
        assert!(write(4, Some(b"hello world"), 0));
        assert!(write(5, None, 10));
        assert_eq!(pipe.q.len(), 5);
        for fd in fds {
            sys::close(fd).unwrap();
        }
        pipe.bus.free_buffer();
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
