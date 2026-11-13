//! `hooks-packet-policy`, ported: the 240 scenarios that ran the pinned
//! `Socket::recv_packet`, `recv_packet_with_length_protocol` and
//! `recv_packet_with_eof_protocol` over a scripted native socket, now run through
//! Rust's framing with the same inputs. `ported.txt` holds the pinned bodies'
//! traces, one line per scenario, without their EOF scans and `String::clear`
//! calls, which Rust performs itself.
use super::{Failure, ReadBuffer, recv_packet};
use crate::co_io::{HOST, Host, errno, set_errno};
use crate::co_socket::{Co, CoSocket, TestSocket};
use core::ffi::c_void;
use core::fmt::Write;
use std::cell::RefCell;

const EIO: isize = libc::EIO as isize;

/// The native read `String`.
struct NativeString {
    bytes: Vec<u8>,
    length: usize,
    offset: usize,
}

/// The scripted socket: each `recv`'s byte count (negative: fail with that errno),
/// the protocol callback's answers, timer allocation, and what happened.
#[derive(Default)]
struct Script {
    trace: Vec<String>,
    lengths: Vec<isize>,
    parsed: usize,
    input: Vec<u8>,
    consumed: usize,
    reads: Vec<isize>,
    read: usize,
    timer_ok: bool,
    available: bool,
    buffer: Option<NativeString>,
}

thread_local! {
    static SCRIPT: RefCell<Script> = RefCell::default();
}

fn with<T>(run: impl FnOnce(&mut Script) -> T) -> T {
    SCRIPT.with_borrow_mut(run)
}

fn timer_add(_: Co, _: f64, _: bool) -> *mut c_void {
    with(|s| {
        s.trace.push("timer:start".into());
        if s.timer_ok {
            core::ptr::dangling_mut::<u64>().cast()
        } else {
            core::ptr::null_mut()
        }
    })
}

unsafe extern "C-unwind" fn timer_del(_: *mut swoole_rs_timer::TimerNode) -> bool {
    true
}

fn read_buffer(_: Co, view: &mut ReadBuffer, action: u32, size: usize) {
    with(|s| {
        if action == 0 {
            s.trace.push("buffer:get".into());
            if s.buffer.is_none() {
                s.trace.push("buffer:new:4".into());
                s.buffer = Some(NativeString {
                    bytes: vec![0; 4],
                    length: 0,
                    offset: 0,
                });
            }
        }
        let buffer = s.buffer.as_mut().unwrap();
        if action != 0 {
            buffer.length = view.length;
            buffer.offset = usize::try_from(view.offset).unwrap();
        }
        if action == 2 {
            s.trace.push(format!("buffer:extend:{size}"));
            assert!(size >= buffer.length, "extend keeps the buffered bytes");
            buffer.bytes.resize(size, 0);
        }
        if action == 3 {
            s.trace.push(format!("buffer:reduce:{size}"));
            assert!(size <= buffer.length, "reduce drops buffered bytes");
            buffer.bytes.copy_within(size..buffer.length, 0);
            buffer.length -= size;
            buffer.offset = 0;
        }
        *view = ReadBuffer {
            length: buffer.length,
            size: buffer.bytes.len(),
            offset: buffer.offset.try_into().unwrap(),
            str: buffer.bytes.as_mut_ptr(),
        };
    });
}

fn recv(_: Co, buf: &mut [u8], all: bool) -> isize {
    let n = buf.len();
    with(|s| {
        let Script {
            trace,
            input,
            consumed,
            reads,
            read,
            ..
        } = s;
        trace.push(format!("{}:{n}", if all { "recv-all" } else { "recv" }));
        let mut total = 0;
        loop {
            let requested = reads.get(*read).copied().unwrap_or(0);
            *read = (*read + 1).min(reads.len());
            if requested < 0 {
                set_errno((-requested).try_into().unwrap());
                return if total > 0 {
                    total.try_into().unwrap()
                } else {
                    -1
                };
            }
            let count = requested
                .unsigned_abs()
                .min(n - total)
                .min(input.len() - *consumed);
            if count == 0 {
                break;
            }
            buf[total..total + count].copy_from_slice(&input[*consumed..*consumed + count]);
            *consumed += count;
            total += count;
            if !all || total >= n {
                break;
            }
        }
        total.try_into().unwrap()
    })
}

fn package_length(_: Co, n: u32, header: &mut u32) -> isize {
    with(|s| {
        s.trace.push(format!("parse:{n}"));
        *header = 4;
        let length = s.lengths[s.parsed.min(s.lengths.len() - 1)];
        s.parsed += 1;
        s.trace.push(format!("trace:{length}:{n}"));
        length
    })
}

fn packet_fail(_: Co, kind: Failure, length: isize) {
    with(|s| {
        let (error, message) = match kind {
            Failure::LengthNotFound => (1781, "get package length failed"),
            Failure::TooLarge => {
                s.trace.extend(["address".into(), "port".into()]);
                s.trace.push(format!("diagnostic:private:9602:{length}"));
                (1782, "native diagnostic")
            }
            Failure::NoEof => (1782, "no package eof, package_max_length exceeded"),
        };
        s.trace.push(format!("error:{error}:{message}"));
        set_errno(error);
    });
}

static SCRIPTED: Host = Host {
    timer_add,
    timer_del,
    read_buffer,
    recv,
    package_length,
    packet_fail,
};

/// The read buffer a scenario starts with: its capacity and buffered prefix.
struct Seed {
    bytes: usize,
    capacity: usize,
}

/// Scenarios 0-95 and 216-227: the length protocol, by mode and variant.
fn length_scenario(s: &mut Script, socket: &mut CoSocket, mode: usize, variant: usize) -> Seed {
    let mut seed = Seed {
        bytes: 0,
        capacity: 4,
    };
    let p = socket.protocol.get_mut();
    socket.open_length_check.set(true);
    socket.open_eof_check.set(variant & 2 != 0);
    s.timer_ok = variant & 4 == 0;
    if variant & 1 != 0 || mode == 11 {
        // A custom length function: no size, no type.
        seed.bytes = 1;
        p.package_length_size = 0;
        p.package_length_type = 0;
    }
    match mode {
        1 => s.lengths = vec![-1],
        2 => (s.lengths, s.reads) = (vec![0, 6], vec![2, 2, 2]),
        3 => s.lengths = vec![24],
        4 => s.lengths = vec![2],
        5 => s.reads = vec![2, 2, 0],
        6 => s.reads = vec![2, -EIO],
        7 => s.reads = vec![0],
        8 => s.reads = vec![-EIO],
        9 => (seed.bytes, seed.capacity) = (6, 8),
        10 => p.package_max_length = 5,
        11 => s.lengths = vec![1],
        _ => {}
    }
    seed
}

/// Scenarios 96-191 and 228-239: the EOF protocol (raw reads when variant & 2).
fn eof_scenario(s: &mut Script, socket: &mut CoSocket, mode: usize, variant: usize) -> Seed {
    let mut seed = Seed {
        bytes: usize::from(variant & 1 != 0),
        capacity: 4,
    };
    let p = socket.protocol.get_mut();
    socket.open_eof_check.set(variant & 2 == 0);
    s.timer_ok = variant & 4 == 0;
    s.input = b"ab\r\ncd".to_vec();
    s.reads = vec![2; 6];
    match mode {
        1 => s.reads = vec![1; 6],
        2 => {
            s.input = b"abcdefghijklmnop".to_vec();
            p.package_max_length = 8;
        }
        3 => (s.input, s.reads) = (b"abcdef\r\n".to_vec(), vec![4, 4]),
        4 => (s.input, seed.bytes, s.reads) = (Vec::new(), 0, vec![0]),
        5 => s.reads = vec![-EIO],
        6 => s.reads = vec![2, -EIO],
        7 => {
            s.input = b"ab\ncd".to_vec();
            p.package_eof[0] = b'\n';
            p.package_eof_len = 1;
        }
        8 => {
            s.input = b"ab\0!cd".to_vec();
            p.package_eof[..2].copy_from_slice(b"\0!");
        }
        9 => (seed.bytes, seed.capacity) = (6, 8),
        10 => {
            (s.input, s.reads) = (b"abcdefghij".to_vec(), vec![4, 4, 4]);
            p.package_max_length = 5;
        }
        11 => {
            (s.input, s.reads) = (b"abcdefghij".to_vec(), vec![4, 4, 4]);
            p.package_eof[..6].copy_from_slice(b"abcdef");
            p.package_eof_len = 6;
        }
        _ => {}
    }
    seed
}

/// Scenarios 192-215: raw reads, with or without a buffer, availability or a timer.
fn raw_scenario(s: &mut Script, k: usize) -> Seed {
    s.input = b"abcdefgh".to_vec();
    s.reads = vec![[3, 0, -EIO][k % 3]];
    s.available = k & 8 == 0;
    s.timer_ok = k & 16 == 0;
    if k & 4 == 0 {
        Seed {
            bytes: 0,
            capacity: 4,
        }
    } else {
        Seed {
            bytes: 2,
            capacity: 8,
        }
    }
}

/// One scenario's trace, as the pinned bodies' facade recorded it.
fn run(scenario: usize) -> String {
    let mut socket = TestSocket::new();
    let record = &mut *socket.0;
    unsafe { (*record.socket.get()).fields.timeouts.read = 1.0 };
    let p = record.protocol.get_mut();
    (p.package_length_offset, p.package_length_size) = (0, 2);
    (p.package_max_length, p.package_length_type) = (16, b'N');
    p.package_eof[..2].copy_from_slice(b"\r\n");
    p.package_eof_len = 2;
    let mut s = Script {
        lengths: vec![6],
        reads: vec![2, 4, 4, 4, 4, 4],
        input: b"abcdefghijklmnop".to_vec(),
        timer_ok: true,
        available: true,
        ..Script::default()
    };
    let (mode, variant) = (scenario % 12, scenario / 12 % 8);
    let seed = match scenario {
        0..96 | 216..228 => length_scenario(&mut s, record, mode, variant),
        192..216 => raw_scenario(&mut s, scenario - 192),
        _ => eof_scenario(&mut s, record, mode, variant),
    };
    if seed.bytes > 0 || scenario >= 216 {
        s.trace.push(format!("buffer:new:{}", seed.capacity));
        let bytes = seed.bytes.min(s.input.len());
        let mut buffer = vec![0; seed.capacity];
        buffer[..bytes].copy_from_slice(&s.input[..bytes]);
        let offset = usize::from((192..216).contains(&scenario) && bytes > 0);
        s.buffer = Some(NativeString {
            bytes: buffer,
            length: bytes,
            offset,
        });
        s.consumed = bytes;
    }
    s.trace.push("available:512".into());
    let available = s.available;
    SCRIPT.set(s);
    set_errno(0);
    let (result, error) = if available {
        with(|s| s.trace.push("timer:new:0x1p+0".into()));
        let previous = HOST.replace(&SCRIPTED);
        let result = recv_packet(socket.co(), 0.0);
        let error = errno();
        HOST.set(previous);
        with(|s| s.trace.push("timer:destroy".into()));
        (result, error)
    } else {
        (-1, 0)
    };
    let mut s = SCRIPT.take();
    s.trace.push(format!("result:{result}"));
    s.trace.push(format!("errno:{error}"));
    if let Some(buffer) = s.buffer.take() {
        let (length, offset, size) = (buffer.length, buffer.offset, buffer.bytes.len());
        s.trace.push(format!("buffer:{length}:{offset}:{size}"));
        let mut bytes = String::from("bytes:");
        for byte in &buffer.bytes[..length] {
            write!(bytes, "{byte:02x}").unwrap();
        }
        s.trace.push(bytes);
        s.trace.push("buffer:delete".into());
    }
    format!("{scenario}: {}", s.trace.join(" | "))
}

fn check(scenarios: impl IntoIterator<Item = usize>) {
    let expected: Vec<&str> = include_str!("ported.txt").lines().collect();
    for scenario in scenarios {
        assert_eq!(run(scenario), expected[scenario]);
    }
}

#[test]
fn ported_length_protocol_scenarios() {
    check((0..96).chain(216..228));
}

#[test]
fn ported_eof_protocol_scenarios() {
    check((96..192).chain(228..240));
}

#[test]
fn ported_raw_receive_scenarios() {
    check(192..216);
}
