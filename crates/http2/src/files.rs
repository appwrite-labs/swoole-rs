//! File/StaticHandler storage, reads and callbacks remain native.
#[must_use]
pub fn sw_rs_http2_file_length(requested: usize, available: usize) -> usize {
    requested.min(available)
}

/// EMPTY=0, MULTI=1, SINGLE=2, SKIP=3. SKIP still opens/closes the native file.
#[must_use]
pub fn sw_rs_http2_file_mode(tasks: usize, first_length: usize) -> u32 {
    match tasks {
        0 => 0,
        1 if first_length == 0 => 3,
        1 => 2,
        _ => 1,
    }
}

/// TRAILER=0, CLOSE=1, REMOVE=2. All three original completion paths return true.
#[must_use]
pub fn sw_rs_http2_file_complete(success: u32, trailer: u32) -> u32 {
    match (success != 0, trailer != 0) {
        (false, _) => 1,
        (true, true) => 0,
        (true, false) => 2,
    }
}
