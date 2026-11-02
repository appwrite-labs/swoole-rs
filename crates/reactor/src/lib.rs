//! Readiness/interest and opt-in metadata ownership; Native retains effects and default registries.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
#[cfg(unix)]
mod wait_loop;
#[cfg(unix)]
pub use wait_loop::*;
#[cfg(unix)]
mod backend;
#[cfg(unix)]
pub use backend::*;
#[cfg(unix)]
mod poll;
#[cfg(unix)]
pub use poll::*;
#[cfg(unix)]
mod write;
#[cfg(unix)]
pub use write::*;
#[cfg(unix)]
mod registration;
#[cfg(unix)]
pub use registration::*;
#[cfg(unix)]
mod registry;
#[cfg(unix)]
pub use registry::*;

#[cfg(unix)]
mod output;
#[cfg(unix)]
pub use output::*;

mod builtins;
pub use builtins::*;

mod initialize;
pub use initialize::*;

mod deferred;
pub use deferred::*;

mod callbacks;
pub use callbacks::*;

mod handlers;
pub use handlers::*;

mod owner;

mod wait;
pub use wait::*;

mod flush;
pub use flush::*;

mod socket_timeouts;
pub use socket_timeouts::*;

#[cfg(unix)]
mod socket_options;
#[cfg(unix)]
pub use socket_options::*;
#[cfg(unix)]
mod socket_io;
#[cfg(unix)]
pub use socket_io::*;
#[cfg(unix)]
mod socket_rw;
#[cfg(unix)]
pub use socket_rw::*;
#[cfg(unix)]
mod socket_sync;
#[cfg(unix)]
pub use socket_sync::*;
#[cfg(unix)]
mod socket_types;
#[cfg(unix)]
pub use socket_types::*;
#[cfg(unix)]
mod socket_record;
#[cfg(unix)]
pub use socket_record::*;

const INVALID: i32 = -1;
/// Encode the stream's four-byte network-order body length. The supplied total
/// length follows native `size_t` subtraction and uint32 truncation semantics.
/// # Safety
/// Destination points to four writable bytes. No pointer is retained.
#[unsafe(no_mangle)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "native uint32_t/int framing conversions, as documented above"
)]
pub unsafe extern "C" fn swr_v1_stream_prepare(destination: *mut u8, total_length: usize) -> i32 {
    if destination.is_null() {
        return INVALID;
    }
    let bytes = (total_length.wrapping_sub(4) as u32).to_be_bytes();
    // SAFETY: non-null (checked above) and covers four writable bytes; `[u8; 4]` has alignment 1.
    unsafe { destination.cast::<[u8; 4]>().write(bytes) };
    0
}
/// Decode sync framing, including the native signed int length/capacity casts.
/// # Safety
/// Header points to four readable bytes, including any native zero-filled tail
/// after a short prefix read. No pointer is retained.
#[unsafe(no_mangle)]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "native uint32_t/int framing conversions, as documented above"
)]
pub unsafe extern "C" fn swr_v1_stream_length(header: *const u8, capacity: usize) -> i32 {
    if header.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above) and covers four readable bytes; `[u8; 4]` has alignment 1.
    let length = u32::from_be_bytes(unsafe { header.cast::<[u8; 4]>().read() }) as i32;
    if length <= 0 || length > capacity as i32 {
        INVALID
    } else {
        length
    }
}
#[repr(C)]
pub struct StreamView {
    pub offset: u32,
    pub length: u32,
    pub close_wait: u32,
}
/// Native dispatch retains the span/callback. A four-byte record terminates;
/// other lengths preserve native uint32 subtraction, including small inputs.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_stream_view(length: u32) -> StreamView {
    StreamView {
        offset: 4,
        length: length.wrapping_sub(4),
        close_wait: u32::from(length == 4),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stream_lengths_preserve_network_bytes_and_native_signed_capacity() {
        let mut bytes = [0xa5; 6];
        assert_eq!(
            unsafe { swr_v1_stream_prepare(bytes.as_mut_ptr().add(1), 12) },
            0
        );
        assert_eq!(bytes, [0xa5, 0, 0, 0, 8, 0xa5]);
        assert_eq!(unsafe { swr_v1_stream_length(bytes.as_ptr().add(1), 8) }, 8);
        assert_eq!(
            unsafe { swr_v1_stream_length(bytes.as_ptr().add(1), i32::MAX as usize + 1) },
            INVALID
        );
        assert_eq!(
            unsafe { swr_v1_stream_prepare(core::ptr::null_mut(), 4) },
            INVALID
        );
        assert_eq!(
            unsafe { swr_v1_stream_length(core::ptr::null(), 8) },
            INVALID
        );
        assert_eq!(swr_v1_stream_view(3).length, u32::MAX);
        assert_eq!(swr_v1_stream_view(4).close_wait, 1);
    }
}

mod iov_progress;
pub use iov_progress::*;

#[cfg(unix)]
mod iov_owned;
#[cfg(unix)]
pub use iov_owned::*;

mod signal_dispatch;
pub use signal_dispatch::*;
