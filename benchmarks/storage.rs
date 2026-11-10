//! Same public-ABI workloads for baseline and candidate; no PHP/native runtime.
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::time::Instant;
use swoole_rs_buffer as buffer;
use swoole_rs_memory as memory;
use swoole_rs_timer as timer;

static TRACK: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
struct Allocator;
fn allocated(size: usize) {
    CALLS.fetch_add(1, Relaxed);
    let live = LIVE.fetch_add(size, Relaxed) + size;
    PEAK.fetch_max(live, Relaxed);
}
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() && TRACK.load(Relaxed) {
            allocated(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        if TRACK.load(Relaxed) {
            LIVE.fetch_sub(layout.size(), Relaxed);
        }
        unsafe { System.dealloc(p, layout) };
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(p, layout, size) };
        if !p.is_null() && TRACK.load(Relaxed) {
            LIVE.fetch_sub(layout.size(), Relaxed);
            allocated(size);
        }
        p
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

fn run(name: &str, rounds: usize) -> u64 {
    let mut sum = 0u64;
    unsafe {
        match name {
            "buffer-1k" | "buffer-16k" | "buffer-cold-1k" => {
                let bytes = vec![0xa5; 65536];
                let mut owner = ptr::null_mut();
                let chunk = if name == "buffer-16k" { 16384 } else { 1024 };
                assert_eq!(buffer::swr_v1_buffer_typed_create(chunk, &mut owner), 0);
                for _ in 0..rounds {
                    assert_eq!(
                        buffer::swr_v1_buffer_append(owner, bytes.as_ptr(), bytes.len()),
                        0
                    );
                    let mut view = buffer::TypedFront::default();
                    while buffer::swr_v1_buffer_typed_front(owner, &mut view) == 0 {
                        sum = sum.wrapping_add(*view.data as u64 + view.length as u64);
                        let mut detached = buffer::TypedDetached::default();
                        assert_eq!(buffer::swr_v1_buffer_typed_pop(owner, &mut detached), 0);
                        assert!(detached.token.is_null());
                    }
                    if name == "buffer-cold-1k" {
                        assert_eq!(buffer::swr_v1_buffer_release(owner), 0);
                        assert_eq!(buffer::swr_v1_buffer_typed_create(chunk, &mut owner), 0);
                    }
                }
                assert_eq!(buffer::swr_v1_buffer_release(owner), 0);
            }
            "heap-1" | "heap-128" | "heap-512" => {
                let n = if name == "heap-1" {
                    1
                } else if name == "heap-128" {
                    128
                } else {
                    512
                };
                let mut owner = ptr::null_mut();
                assert_eq!(memory::swr_v1_memory_heap_create(n, 0, &mut owner), 0);
                for _ in 0..rounds {
                    for id in 1..=n {
                        assert!(
                            !memory::swr_v1_memory_heap_push(
                                owner,
                                (n - id) as u64,
                                ptr::without_provenance_mut(id)
                            )
                            .is_null()
                        );
                    }
                    for id in (1..=n).rev() {
                        let value = memory::swr_v1_memory_heap_pop(owner).addr();
                        assert_eq!(value, id);
                        sum += value as u64;
                    }
                }
                memory::swr_v1_memory_heap_release(owner);
            }
            "timer-churn-512" => {
                let mut owner = ptr::null_mut();
                assert_eq!(timer::swr_v1_timer_create(&mut owner), 0);
                let mut view = timer::TimerView::default();
                for round in 0..rounds {
                    for id in 1..=512 {
                        assert_eq!(
                            timer::swr_v1_timer_add(
                                owner,
                                0,
                                513 - id,
                                0,
                                ptr::without_provenance_mut(id as usize),
                                &mut view
                            ),
                            0
                        );
                    }
                    for id in 1..=512 {
                        assert_eq!(
                            timer::swr_v1_timer_remove(owner, (round * 512 + id) as i64, &mut view),
                            3
                        );
                        assert_eq!(view.token.addr(), id);
                        sum += view.deadline as u64;
                    }
                }
                assert_eq!(timer::swr_v1_timer_release(owner), 0);
            }
            "timer-512" | "timer-4096" => {
                let n = if name == "timer-512" { 512 } else { 4096 };
                let mut owner = ptr::null_mut();
                assert_eq!(timer::swr_v1_timer_create(&mut owner), 0);
                let mut view = timer::TimerView::default();
                for id in 1..=n {
                    assert_eq!(
                        timer::swr_v1_timer_add(
                            owner,
                            0,
                            id as i64,
                            0,
                            ptr::without_provenance_mut(id),
                            &mut view
                        ),
                        0
                    );
                }
                for round in 0..rounds {
                    for id in 1..=n {
                        let deadline = ((id * 7919 + round * 17) % (n * 4)) as i64;
                        assert_eq!(
                            timer::swr_v1_timer_update(owner, id as i64, deadline, &mut view),
                            0
                        );
                        assert_eq!(view.id, id as i64);
                        assert_eq!(view.deadline, deadline);
                        sum = sum.wrapping_add(view.deadline as u64);
                    }
                }
                for id in 1..=n {
                    assert_eq!(timer::swr_v1_timer_remove(owner, id as i64, &mut view), 3);
                    assert_eq!(view.token.addr(), id);
                }
                assert_eq!(timer::swr_v1_timer_release(owner), 0);
            }
            _ => panic!("unknown workload"),
        }
    }
    black_box(sum)
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let name = &args[1];
    let rounds: usize = args[2].parse().unwrap();
    assert!(rounds > 0);
    // Allocation probes run separately so accounting atomics are outside timing.
    if args.get(3).map(String::as_str) == Some("alloc") {
        TRACK.store(true, Relaxed);
        let checksum = run(name, rounds);
        TRACK.store(false, Relaxed);
        assert_eq!(LIVE.load(Relaxed), 0, "workload leaked Rust allocations");
        println!(
            "{{\"checksum\":{checksum},\"allocations\":{},\"peak_requested_bytes\":{}}}",
            CALLS.load(Relaxed),
            PEAK.load(Relaxed)
        );
    } else {
        run(name, rounds.min(10));
        let start = Instant::now();
        let checksum = run(name, rounds);
        let ns = start.elapsed().as_nanos();
        println!("{{\"checksum\":{checksum},\"nanoseconds\":{ns}}}");
    }
}
