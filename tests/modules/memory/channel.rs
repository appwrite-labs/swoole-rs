use super::*;
use std::collections::VecDeque;
use std::ptr;
use swoole_rs_corebase::CCast;
#[test]
fn channel_fifo_spill_wrap_peek_and_capacity_match_independent_model() {
    let mut memory = [0u8; 128 + 24 + 4];
    let mut c = Channel {
        head: 0,
        tail: 0,
        size: 128,
        head_tag: 0,
        tail_tag: 0,
        num: 0,
        max_num: 0,
        bytes: 0,
        flags: 0,
        maxlen: 24,
        mem: memory.as_mut_ptr(),
    };
    let mut model = VecDeque::<Vec<u8>>::new();
    let mut seed = 42u64;
    for _ in 0..20000 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        if seed & 3 != 0 {
            let bytes = vec![seed.c_cast::<u8>(); (seed % 25) as usize];
            if swr_v1_memory_channel_in(&mut c, &bytes) == 0 {
                model.push_back(bytes);
            }
        } else {
            let mut output = [0u8; 24];
            let consume = ((seed >> 2) & 1) as u32;
            let capacity = if seed & 32 != 0 { 24 } else { 0 };
            let status = swr_v1_memory_channel_out(
                &mut c,
                &mut output[..capacity.c_cast::<usize>()],
                consume != 0,
            );
            if let Some(front) = model.front() {
                if front.len() <= capacity.c_cast::<usize>() {
                    assert_eq!(status, front.len().c_cast::<i32>());
                    assert_eq!(&output[..front.len()], front);
                    if consume != 0 {
                        model.pop_front();
                    }
                } else {
                    assert_eq!(status, -1);
                }
            } else {
                assert_eq!(status, -1);
            }
        }
        assert_eq!(c.num.c_cast::<usize>(), model.len());
        assert_eq!(c.bytes, model.iter().map(Vec::len).sum());
    }
}

#[test]
fn invalid_wrap_tags_reject_push_peek_and_pop_without_writes() {
    fn state(c: &Channel) -> (i64, i64, u8, u8, i32, usize) {
        (c.head, c.tail, c.head_tag, c.tail_tag, c.num, c.bytes)
    }
    for head_tag in [0, 1, 2, u8::MAX] {
        for tail_tag in [0, 1, 2, u8::MAX] {
            if head_tag <= 1 && tail_tag <= 1 {
                continue;
            }
            let mut memory = [0xabu8; 16];
            let mut c = Channel {
                head: 0,
                tail: 7,
                size: 8,
                head_tag,
                tail_tag,
                num: 0,
                max_num: 0,
                bytes: 0,
                flags: 0,
                maxlen: 4,
                mem: memory.as_mut_ptr(),
            };
            let before = state(&c);
            let byte = 42u8;
            assert_eq!(swr_v1_memory_channel_in(&mut c, &[byte]), -1);
            assert_eq!(state(&c), before);
            assert_eq!(memory, [0xab; 16]);

            // An otherwise readable record crosses the logical end, exercising
            // both the peek path and the consuming head-tag wrap path.
            c.head = 7;
            c.tail = 0;
            c.num = 1;
            c.bytes = 1;
            memory[7..11].copy_from_slice(&1i32.to_ne_bytes());
            memory[11] = byte;
            let before_memory = memory;
            let before = state(&c);
            for consume in [false, true] {
                let mut output = [0xcdu8; 4];
                assert_eq!(swr_v1_memory_channel_out(&mut c, &mut output, consume), -1);
                assert_eq!(state(&c), before);
                assert_eq!(memory, before_memory);
                assert_eq!(output, [0xcd; 4]);
            }
        }
    }
}

#[test]
fn invalid_configuration_is_rejected_before_accessing_queue_storage() {
    for case in 0..12 {
        let mut memory = [0xabu8; 16];
        // Valid for both push and output before corrupting one boundary.
        memory[..4].copy_from_slice(&1i32.to_ne_bytes());
        let mut c = Channel {
            head: 0,
            tail: 5,
            size: 8,
            head_tag: 0,
            tail_tag: 0,
            num: 1,
            max_num: 0,
            bytes: 1,
            flags: 0,
            maxlen: 4,
            mem: memory.as_mut_ptr(),
        };
        match case {
            0 => c.mem = ptr::null_mut(),
            1 => c.size = 0,
            2 => c.size = usize::MAX,
            3 => c.size = isize::MAX as usize,
            4 => c.head = -1,
            5 => c.tail = -1,
            6 => c.head = 8,
            7 => c.tail = 8,
            8 => c.num = -1,
            9 => c.maxlen = -1,
            10 => c.maxlen = 9,
            11 => c.head = i64::MIN,
            _ => unreachable!(),
        }
        let before_memory = memory;
        let before = (c.head, c.tail, c.head_tag, c.tail_tag, c.num, c.bytes);
        let input = 42u8;
        assert_eq!(
            swr_v1_memory_channel_in(&mut c, &[input]),
            -1,
            "push boundary {case}"
        );
        for consume in [false, true] {
            let mut output = [0xcdu8; 4];
            assert_eq!(
                swr_v1_memory_channel_out(&mut c, &mut output, consume),
                -1,
                "output boundary {case}"
            );
            assert_eq!(output, [0xcd; 4]);
        }
        assert_eq!(memory, before_memory);
        assert_eq!(
            (c.head, c.tail, c.head_tag, c.tail_tag, c.num, c.bytes),
            before
        );
    }
}

#[test]
fn every_short_copy_width_and_spilled_record_preserves_exact_payload_and_boundaries() {
    for length in 0..=256usize {
        // Exact input/output allocations let ASan detect endpoint overreads,
        // including lengths just below/above each fixed-width copy threshold.
        let input: Vec<u8> = (0..length)
            .map(|i| (i.wrapping_mul(37) ^ length).c_cast::<u8>())
            .collect();
        for cursor in [0usize, 3, 127] {
            let mut memory = vec![0xabu8; 128 + 256 + 4];
            let mut c = Channel {
                head: cursor.c_cast::<i64>(),
                tail: cursor.c_cast::<i64>(),
                size: 128,
                head_tag: 0,
                tail_tag: 0,
                num: 0,
                max_num: 0,
                bytes: 0,
                flags: 0,
                maxlen: 128,
                mem: memory.as_mut_ptr(),
            };
            // Native channels require maxlen <= logical size; use a larger
            // logical ring for large messages while retaining the end spill.
            if length > 128 {
                c.size = 256;
                c.maxlen = 256;
                c.head += 128;
                c.tail += 128;
                memory.resize(256 + 256 + 4, 0xab);
                c.mem = memory.as_mut_ptr();
            }
            let start = c.tail.c_cast::<usize>();
            assert_eq!(swr_v1_memory_channel_in(&mut c, &input), 0);
            assert_eq!(&memory[..start], vec![0xab; start]);
            assert_eq!(&memory[start + 4..start + 4 + length], input);
            assert!(
                memory[start + 4 + length..]
                    .iter()
                    .all(|byte| *byte == 0xab)
            );
            let before = (c.head, c.tail, c.head_tag, c.tail_tag, c.num, c.bytes);
            for consume in [false, true] {
                let mut output = vec![0xcdu8; length];
                assert_eq!(
                    swr_v1_memory_channel_out(&mut c, &mut output, consume),
                    length.c_cast::<i32>()
                );
                assert_eq!(output, input);
                if !consume {
                    assert_eq!(
                        (c.head, c.tail, c.head_tag, c.tail_tag, c.num, c.bytes),
                        before
                    );
                }
            }
            assert_eq!(c.num, 0);
            assert_eq!(c.bytes, 0);
            assert_eq!(c.head, c.tail);
            assert_eq!(c.head_tag, c.tail_tag);
        }
    }
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no pthread_mutexattr_setpshared")]
fn record_locks_signals_and_prints_like_native_channel() {
    for flags in [0, LOCK, NOTIFY, LOCK | NOTIFY] {
        let bytes = swr_v1_memory_channel_bytes(128, 32);
        assert_eq!(bytes, size_of::<Record>() + 128 + 32 + 4);
        let mut memory = vec![0u64; bytes.div_ceil(8)];
        let record = memory.as_mut_ptr().cast::<Record>();
        assert_eq!(
            unsafe { swr_v1_memory_channel_init(record, 128, 32, flags) },
            0
        );
        let live = unsafe { &*record };
        assert_eq!(live.queue(|queue| queue.mem), record.wrapping_add(1).cast());
        assert_eq!(live.push(b"life"), 0);
        let mut output = [0u8; 32];
        assert_eq!(live.pop(&mut output, false), 4);
        assert_eq!(live.pop(&mut output, true), 4);
        assert_eq!(&output[..4], b"life");
        if flags & NOTIFY != 0 {
            assert_eq!(live.signal(true), 0, "notify");
            assert_eq!(live.signal(false), 0, "wait");
        }
        unsafe { swr_v1_memory_channel_close(record) };
    }
    assert_eq!(swr_v1_memory_channel_bytes(8, 16), 0, "maxlen above size");
    assert_eq!(swr_v1_memory_channel_bytes(usize::MAX, 4), 0, "overflow");
    assert_eq!(
        unsafe { swr_v1_memory_channel_init(ptr::null_mut(), 128, 32, 0) },
        -1
    );
}

#[test]
fn public_capacity_predicate_preserves_defined_modular_scalar_domain() {
    assert_eq!(std::mem::size_of::<CapacityState>(), 40);
    for head in [-1, 0, 1, i64::MAX] {
        for tail in [-1, 0, 1, i64::MAX] {
            for (head_tag, tail_tag) in [(0, 0), (0, 1), (1, 0), (127, 255), (255, 255)] {
                for bytes in [0, 1, 4, usize::MAX - 3, usize::MAX] {
                    for num in [0, 1, -1, i32::MIN, i32::MAX] {
                        let modular = ((bytes as u128 + (num.c_cast::<usize>() as u128) * 4)
                            & usize::MAX as u128) as usize;
                        for size in [0, 4, 16, usize::MAX, modular] {
                            let state = CapacityState {
                                head,
                                tail,
                                size,
                                bytes,
                                num,
                                head_tag,
                                tail_tag,
                            };
                            let expected =
                                (head == tail && head_tag != tail_tag) || modular == size;
                            assert_eq!(
                                swr_v1_memory_channel_full(Some(&state)),
                                i32::from(expected)
                            );
                        }
                    }
                }
            }
        }
    }
    assert_eq!(swr_v1_memory_channel_full(None), -1);
}

/// `swoole::Channel` as upstream's `include/swoole_channel.h` drove the record: `make`, the
/// locked `push`, `pop` and `peek`, `wait`, `notify` and `destroy`.
struct Native(Vec<u64>);

impl Native {
    fn make(size: usize, maxlen: usize, flags: i32) -> Self {
        let total = swr_v1_memory_channel_bytes(size, maxlen);
        let mut memory = vec![0u64; total.div_ceil(8)];
        let record = memory.as_mut_ptr().cast::<Record>();
        assert_eq!(
            unsafe { swr_v1_memory_channel_init(record, size, maxlen, flags) },
            0
        );
        Native(memory)
    }
    fn record(&self) -> *mut Record {
        self.0.as_ptr().cast::<Record>().cast_mut()
    }
    fn live(&self) -> &Record {
        unsafe { &*self.record() }
    }
    fn push(&self, data: &[u8]) -> i32 {
        self.live().push(data)
    }
    /// `pop` (`consume`) or `peek`: -1 (`SW_ERR`) when empty.
    fn take(&self, buf: &mut [u8], consume: u32) -> i32 {
        self.live().pop(buf, consume != 0)
    }
    fn wait(&self) -> i32 {
        self.live().signal(false)
    }
    fn notify(&self) -> i32 {
        self.live().signal(true)
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        unsafe { swr_v1_memory_channel_close(self.record()) };
    }
}

// The record's mutex serializes the queue, as upstream's does.
unsafe impl Sync for Native {}

const SW_CHAN_LOCK: i32 = 2;
const SW_CHAN_NOTIFY: i32 = 4;

/// Upstream core-tests/src/core/channel.cpp `TEST(channel, push)`: 10 MB of random
/// 4000..7999-byte records through a 128 KiB channel, one thread pushing, one popping
/// and comparing each record with the one pushed.
#[test]
#[cfg_attr(miri, ignore = "Miri has no pthread_mutexattr_setpshared")]
fn channel_push() {
    const N: usize = 10_000_000;
    let c = Native::make(128 * 1024, 8192, SW_CHAN_LOCK | SW_CHAN_NOTIFY);
    let mut m = Vec::new();
    let mut bytes = 0;
    while bytes < N {
        let n = ((unsafe { libc::rand() }).c_cast::<usize>() % 4000) + 4000;
        let buf: Vec<u8> = (0..n)
            .map(|_| (unsafe { libc::rand() }).c_cast::<u8>())
            .collect();
        bytes += n;
        m.push(buf);
    }
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut bytes = 0;
            let mut next = m.iter().peekable();
            while let Some(data) = next.peek() {
                if bytes >= N {
                    break;
                }
                if c.push(data) == 0 {
                    bytes += data.len();
                    next.next();
                } else {
                    std::thread::sleep(std::time::Duration::from_micros(10));
                }
            }
        });
        scope.spawn(|| {
            let mut buf = [0u8; 8000];
            let mut bytes = 0;
            let mut index = 0;
            while bytes < N {
                let retval = c.take(&mut buf, 1);
                if retval > 0 {
                    assert_eq!(m[index], buf[..retval.c_cast::<usize>()]);
                    index += 1;
                    bytes += retval.c_cast::<usize>();
                } else {
                    std::thread::sleep(std::time::Duration::from_micros(10));
                }
            }
        });
    });
}

/// `TEST(channel, peek)`.
#[test]
#[cfg_attr(miri, ignore = "Miri has no pthread_mutexattr_setpshared")]
fn channel_peek() {
    let mut buf = [0u8; 8000];
    let c = Native::make(128 * 1024, 8192, SW_CHAN_LOCK | SW_CHAN_NOTIFY);
    assert_eq!(c.take(&mut buf, 0), -1);
    let value = b"test";
    c.push(value);
    assert_eq!(c.take(&mut buf, 0), value.len().c_cast::<i32>());
}

/// `TEST(channel, notify)`. Upstream's `sleep(0.02)` truncates to `sleep(0)`, and its
/// `while (c->wait())` pops only when `wait` fails; both are kept as they are.
#[test]
#[cfg_attr(miri, ignore = "Miri has no pthread_mutexattr_setpshared")]
fn channel_notify() {
    let c = Native::make(128 * 1024, 8192, SW_CHAN_LOCK | SW_CHAN_NOTIFY);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            c.push(b"test");
            c.notify();
        });
        scope.spawn(|| {
            #[expect(clippy::never_loop, reason = "upstream's loop, kept as it is")]
            while c.wait() != 0 {
                let mut buf = [0u8; 8000];
                assert!(c.take(&mut buf, 1) > 0);
                break;
            }
        });
    });
    c.live().print();
}
