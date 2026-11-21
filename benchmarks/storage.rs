//! Same workloads for baseline and candidate; no PHP/native runtime.
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::time::Instant;
use swoole_rs_buffer as buffer;
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
            "timer-churn-512" => {
                let mut owner = timer::Timer::default();
                for round in 0..rounds {
                    for id in 1..=512 {
                        let token = ptr::without_provenance_mut(id as usize);
                        assert_eq!(owner.add(0, 513 - id, false, token).0, 0);
                    }
                    for id in 1..=512 {
                        let (status, view) = owner.remove((round * 512 + id) as i64);
                        assert_eq!(status, 3);
                        assert_eq!(view.token.addr(), id);
                        sum += view.deadline as u64;
                    }
                }
                assert!(owner.idle());
            }
            "timer-512" | "timer-4096" => {
                let n = if name == "timer-512" { 512 } else { 4096 };
                let mut owner = timer::Timer::default();
                for id in 1..=n {
                    let token = ptr::without_provenance_mut(id);
                    assert_eq!(owner.add(0, id as i64, false, token).0, 0);
                }
                for round in 0..rounds {
                    for id in 1..=n {
                        let deadline = ((id * 7919 + round * 17) % (n * 4)) as i64;
                        let (status, view) = owner.update(id as i64, deadline);
                        assert_eq!(status, 0);
                        assert_eq!(view.id, id as i64);
                        assert_eq!(view.deadline, deadline);
                        sum = sum.wrapping_add(view.deadline as u64);
                    }
                }
                for id in 1..=n {
                    let (status, view) = owner.remove(id as i64);
                    assert_eq!(status, 3);
                    assert_eq!(view.token.addr(), id);
                }
                assert!(owner.idle());
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
