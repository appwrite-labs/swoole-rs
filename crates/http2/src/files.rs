//! Completion of an HTTP/2 file response.

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
