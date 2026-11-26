//! Allocation bounds for the decode targets (#5707): a counting global allocator, and
//! for each target the most it may hold live while it runs: `k` bytes per input byte
//! plus `c`, where `c` carries any output cap the target hands its decoder.
//!
//! The counter tracks peak live bytes, not the total allocated: a buffer that grows by
//! doubling or a per-chunk scratch `Vec` freed each round costs a server nothing it
//! keeps, while what is live at once is what a peer can make a connection hold.
//! It sees only Rust's allocations. zlib, Brotli, zstd, nghttp2 and the oracle's C++
//! call `malloc` themselves; libFuzzer's `-rss_limit_mb` is what bounds those.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use swoole_rs_http2::hpack::MAX_DECODED_STRING;

const KIB: usize = 1024;

thread_local! {
    /// Bytes this thread allocated less those it freed, and the highest that reached.
    static LIVE: Cell<(isize, isize)> = const { Cell::new((0, 0)) };
}

/// Moves this thread's live count by `delta` and raises its peak.
fn count(delta: isize) {
    // A thread past its thread-locals' teardown goes uncounted.
    let _ = LIVE.try_with(|live| {
        let (now, peak) = live.get();
        let now = now.wrapping_add(delta);
        live.set((now, peak.max(now)));
    });
}

/// [`System`], counting each thread's live bytes.
struct Counting;

// SAFETY: every method forwards to `System` with its caller's arguments and returns
// what `System` returned; counting touches only a thread-local `Cell`.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `alloc`'s contract.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            count(layout.size().cast_signed());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `alloc_zeroed`'s contract.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            count(layout.size().cast_signed());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `dealloc`'s contract.
        unsafe { System.dealloc(ptr, layout) };
        count(-layout.size().cast_signed());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `realloc`'s contract.
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            count(
                new_size
                    .cast_signed()
                    .wrapping_sub(layout.size().cast_signed()),
            );
        }
        new
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The output cap `ws_inflate` hands `swoole_rs_inflate::message` for any bytes.
pub(crate) const WS_INFLATE_CAP: usize = 64 * KIB;

/// A decode target, its entry point, `k` (bytes per input byte) and `c` (bytes).
type Bound = (&'static str, fn(&[u8]), usize, usize);

/// Bounds from each target's peak on its seeds and on generated inputs up to 1 MiB,
/// with about twice the headroom (the PR that added this lists them). A cap a target
/// hands its decoder is in its bound: the inflate targets cap output at the input's
/// length (`k`), `ws_inflate`'s first call at [`WS_INFLATE_CAP`] and HPACK each string
/// at `MAX_DECODED_STRING` (`c`). `resp_stream` and `framing_stream` hold the harness's
/// 64 KiB stream logs; the RESP targets' `k` is mostly the harness's item encoding.
const BOUNDS: [Bound; 13] = [
    (
        "ws_inflate",
        crate::ws_inflate,
        4,
        WS_INFLATE_CAP + 16 * KIB,
    ),
    ("http_inflate", crate::http_inflate, 8, 16 * KIB),
    ("http_decompress", crate::http_decompress, 8, 16 * KIB),
    (
        "http2_hpack",
        crate::http2_hpack,
        2,
        2 * MAX_DECODED_STRING + 16 * KIB,
    ),
    ("http2_frame", crate::http2_frame, 1, 4 * KIB),
    ("resp_parse", crate::resp_parse, 10, 4 * KIB),
    ("resp_server", crate::resp_server, 16, 4 * KIB),
    ("resp_stream", crate::resp_stream, 2, 128 * KIB),
    ("framing_length", crate::framing_length, 1, 4 * KIB),
    ("framing_stream", crate::framing_stream, 2, 256 * KIB),
    ("framing_unpack", crate::framing_unpack, 1, 4 * KIB),
    ("http_multipart", crate::multipart, 1, 4 * KIB),
    ("http_chunked", crate::http_chunked, 2, 4 * KIB),
];

/// Runs `target` over `bytes` and returns the most it held live at once.
fn peak_live(target: fn(&[u8]), bytes: &[u8]) -> usize {
    let (start, _) = LIVE.get();
    LIVE.set((start, start));
    target(bytes);
    let (_, peak) = LIVE.get();
    peak.wrapping_sub(start).cast_unsigned()
}

/// The fuzz entry point of a decode target: runs it over `bytes` and fails when it held
/// more than `k * bytes.len() + c` live.
///
/// # Panics
///
/// When `target` has no bound, or held more than it.
pub fn bounded(target: &str, bytes: &[u8]) {
    let &(_, run, k, c) = BOUNDS
        .iter()
        .find(|bound| bound.0 == target)
        .expect("a target in BOUNDS");
    let peak = peak_live(run, bytes);
    let bound = k * bytes.len() + c;
    assert!(
        peak <= bound,
        "{target} held {peak} bytes live over {} input bytes, past {bound}",
        bytes.len()
    );
}

/// Replays each decode target's seeds (`fuzz/seeds`) through [`bounded`], printing the
/// most any seed held live against the bound for that seed.
#[cfg(test)]
mod tests {
    #[test]
    fn seeds_stay_bounded() {
        let seeds = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("seeds");
        for (target, run, k, c) in super::BOUNDS {
            let mut worst = (0, 0);
            for entry in std::fs::read_dir(seeds.join(target)).unwrap() {
                let bytes = std::fs::read(entry.unwrap().path()).unwrap();
                worst = worst.max((super::peak_live(run, &bytes), k * bytes.len() + c));
                super::bounded(target, &bytes);
            }
            eprintln!("{target}: peak {} bytes live, bound {}", worst.0, worst.1);
        }
    }
}
