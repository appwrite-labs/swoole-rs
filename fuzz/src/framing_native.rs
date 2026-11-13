//! `swr_v1_framing_recv` (native/framing/swoole_rs_framing.h): the receive loop for
//! the oracle's native candidate from `crates/oracle/candidate/src/protocol/base.cc`,
//! which the `framing_stream` target runs against the pinned base.cc. `io` holds
//! the arguments and `call(context, op, arg)` performs each effect and refreshes
//! `io.flags`. The hybrid runs the loop over crates/ports' effects instead.
use std::ffi::c_void;
use swoole_rs_framing::recv::{self, Buffer, Effects, FLAG_REMOVED, Mode, Op, RECV_CLOSE, recv};

const SW_ERR: i32 = -1;

/// The head of `swoole::String`.
#[repr(C)]
pub struct StringHead {
    pub length: usize,
    pub size: usize,
    pub offset: i64,
    pub str: *mut u8,
}

/// The leading fields of `swoole::Protocol` (`split_by_eof` is a C++ `bool`).
#[repr(C)]
pub struct ProtocolHead {
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
    pub protocol: *const ProtocolHead,
    pub buffer: *mut StringHead,
    pub mode: u32,
    pub head: u32,
    pub flags: u32,
}

/// One native effect on the socket, buffer or protocol in `context`.
pub type FramingCall = unsafe extern "C" fn(context: *mut c_void, op: u32, arg: u64) -> i64;

/// `SWR_FRAMING_OP_*` for `op`. The native effect tables predate the reset, which
/// only changes how the connection closes: it has none.
fn code(op: Op) -> Option<u32> {
    Some(match op {
        Op::Recv => 1,
        Op::GetLength => 2,
        Op::Extend => 3,
        Op::Dispatch => 4,
        Op::Reduce => 5,
        Op::Split => 7,
        Op::SslPending => 8,
        Op::ClearSkip => 9,
        Op::Page => 10,
        Op::LogMalformed => 11,
        Op::LogNoLength => 12,
        Op::LogTooLarge => 13,
        Op::Reset => return None,
    })
}

/// Any op: a native exception was caught; native code rethrows it after return.
pub const THROWN: i64 = i64::MIN;

struct NativeEffects {
    io: *mut SwrFramingIo,
    protocol: *const ProtocolHead,
    buffer: *mut StringHead,
    context: *mut c_void,
    call: FramingCall,
    thrown: bool,
}

impl Effects for NativeEffects {
    /// After a `THROWN` op, no further effect runs: each op fails and the socket
    /// reads as removed, so the loop returns within a few steps.
    fn effect(&mut self, op: Op, arg: u64, flags: &mut u32) -> i64 {
        let Some(op) = code(op) else {
            return 0;
        };
        if !self.thrown {
            // SAFETY: the caller's live receive; native code reads it only inside `call`.
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
    fn buffer(&self) -> Buffer {
        // SAFETY: the caller's live String, changed only inside `call`.
        let b = unsafe { &*self.buffer };
        Buffer {
            length: b.length,
            size: b.size,
            offset: b.offset,
        }
    }
    fn set_buffer(&mut self, length: usize, offset: i64) {
        // SAFETY: as in `buffer`.
        let b = unsafe { &mut *self.buffer };
        b.length = length;
        b.offset = offset;
    }
    fn data(&self) -> &[u8] {
        // SAFETY: as in `buffer`.
        let b = unsafe { &*self.buffer };
        if b.length == 0 {
            return &[];
        }
        // SAFETY: `str` holds `length` received bytes.
        unsafe { std::slice::from_raw_parts(b.str, b.length) }
    }
    fn protocol(&self) -> recv::Protocol {
        // SAFETY: the caller's live protocol.
        let p = unsafe { &*self.protocol };
        recv::Protocol {
            split_by_eof: p.split_by_eof != 0,
            package_eof: p.package_eof,
            package_eof_len: p.package_eof_len,
            package_length_offset: p.package_length_offset,
            package_max_length: p.package_max_length,
        }
    }
}

/// Runs one receive of `io.mode` and returns its `SW_*` result. After a `THROWN`
/// op it runs no further effect; native code rethrows.
///
/// # Safety
/// `io` is null or the caller's live receive, whose protocol and buffer `call`
/// alone changes.
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
    let mode = match shared.mode {
        0 => Mode::Length,
        1 => Mode::Eof,
        2 => Mode::Split,
        _ => return SW_ERR,
    };
    let (head, mut flags) = (shared.head, shared.flags);
    let mut effects = NativeEffects {
        io,
        protocol: shared.protocol,
        buffer: shared.buffer,
        context,
        call,
        thrown: false,
    };
    let status = recv(mode, head, &mut flags, &mut effects);
    // SAFETY: the caller's live receive.
    unsafe { (*io).flags = flags };
    if effects.thrown { SW_ERR } else { status as i32 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;
    const OP_RECV: u32 = 1;
    const OP_GET_LENGTH: u32 = 2;
    const OP_EXTEND: u32 = 3;

    struct Fake {
        protocol: ProtocolHead,
        buffer: StringHead,
        storage: Vec<u8>,
        events: Vec<u32>,
    }

    /// Reads one length byte 5, then throws at the extend it asks for.
    unsafe extern "C" fn throwing(context: *mut c_void, op: u32, _: u64) -> i64 {
        // SAFETY: the test passes its live fake as the context.
        let fake = unsafe { &mut *context.cast::<Fake>() };
        fake.events.push(op);
        match op {
            OP_RECV => {
                fake.storage[0] = 5;
                1
            }
            OP_GET_LENGTH => 6,
            OP_EXTEND => THROWN,
            _ => 0,
        }
    }

    #[test]
    fn a_thrown_effect_runs_no_further_effect() {
        let mut fake = Box::new(Fake {
            protocol: ProtocolHead {
                split_by_eof: 0,
                package_eof: [0; 8],
                package_eof_len: 0,
                package_length_type: b'C',
                package_length_size: 1,
                package_length_offset: 0,
                package_body_offset: 1,
                package_max_length: 64,
            },
            buffer: StringHead {
                length: 0,
                size: 2,
                offset: 0,
                str: ptr::null_mut(),
            },
            storage: vec![0; 2],
            events: Vec::new(),
        });
        fake.buffer.str = fake.storage.as_mut_ptr();
        // One pointer for the context and the fields `io` aliases.
        let raw = ptr::from_mut(&mut *fake);
        let mut io = SwrFramingIo {
            protocol: unsafe { &raw const (*raw).protocol },
            buffer: unsafe { &raw mut (*raw).buffer },
            mode: 0,
            head: 1,
            flags: 0,
        };
        let status = unsafe { swr_v1_framing_recv(&raw mut io, raw.cast(), Some(throwing)) };
        assert_eq!(status, SW_ERR);
        assert_eq!(fake.events, [OP_RECV, OP_GET_LENGTH, OP_EXTEND]);
        let status = unsafe { swr_v1_framing_recv(ptr::null_mut(), raw.cast(), Some(throwing)) };
        assert_eq!(status, SW_ERR);
    }
}
