//! File/StaticHandler storage, reads and callbacks remain native.
#[must_use]
pub fn sw_rs_http2_file_length(requested: usize, available: usize) -> usize {
    requested.min(available)
}

/// How a file response sends its ranges. `Skip` still opens and closes the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileMode {
    Empty,
    Multi,
    Single,
    Skip,
}

#[must_use]
pub fn sw_rs_http2_file_mode(tasks: usize, first_length: usize) -> FileMode {
    match tasks {
        0 => FileMode::Empty,
        1 if first_length == 0 => FileMode::Skip,
        1 => FileMode::Single,
        _ => FileMode::Multi,
    }
}

/// A file response's next step. All three original completion paths return true.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileCompletion {
    Trailer,
    Close,
    Remove,
}

#[must_use]
pub fn sw_rs_http2_file_complete(success: bool, trailer: bool) -> FileCompletion {
    match (success, trailer) {
        (false, _) => FileCompletion::Close,
        (true, true) => FileCompletion::Trailer,
        (true, false) => FileCompletion::Remove,
    }
}
