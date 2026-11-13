//! Brotli over the system libbrotli that swoole.so links, as upstream's does:
//! the server's one-shot compression (`HttpContext::compress`), the client's
//! streaming decoder (`Client::decompress_response`) and MINFO's versions. Every
//! call into libbrotli is here, so crates/inflate, which handles untrusted bytes,
//! forbids `unsafe`.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use std::ffi::{c_int, c_void};
use std::marker::PhantomData;
use std::ptr::{NonNull, null_mut};
use swoole_rs_corebase::sys::Text;

/// `BROTLI_MIN_QUALITY`, `BROTLI_MAX_QUALITY`.
pub const MIN_QUALITY: i32 = 0;
pub const MAX_QUALITY: i32 = 11;
/// `BROTLI_DEFAULT_WINDOW` and `BROTLI_DEFAULT_MODE` (`BROTLI_MODE_GENERIC`).
const DEFAULT_WINDOW: c_int = 22;
const DEFAULT_MODE: c_int = 0;
const BROTLI_TRUE: c_int = 1;
/// `BrotliDecoderResult`'s `SUCCESS`, `NEEDS_MORE_INPUT` and `NEEDS_MORE_OUTPUT`.
const SUCCESS: c_int = 1;
const NEEDS_MORE_INPUT: c_int = 2;
const NEEDS_MORE_OUTPUT: c_int = 3;
/// How much the decoder's output grows by when libbrotli asks for more room.
const BUFFER: usize = 16384;

/// The opaque `BrotliDecoderState`.
type State = c_void;
/// A decoder state `BrotliDecoderCreateInstance` made, live until the unsafe
/// `BrotliDecoderDestroyInstance` of it, whose caller drops the owner unused. It is
/// not `Send`, as the pointer it replaced was not.
#[repr(transparent)]
struct Owned(NonNull<State>);

impl Owned {
    /// The state, for the calls that stay unsafe.
    const fn as_ptr(&self) -> *mut State {
        self.0.as_ptr()
    }

    /// The state while its owner is borrowed, for the safe calls.
    const fn live(&self) -> Live<'_> {
        Live(self.0, PhantomData)
    }
}

/// An [`Owned`] state, borrowed: what a safe libbrotli call takes.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Live<'a>(NonNull<State>, PhantomData<&'a Owned>);
/// Where libbrotli takes a custom allocator's `alloc`, `free` or `opaque`: always
/// null here (nothing makes the reference), which selects malloc and free.
type DefaultAllocator = Option<&'static std::convert::Infallible>;

unsafe extern "C" {
    safe fn BrotliEncoderVersion() -> u32;
    safe fn BrotliDecoderVersion() -> u32;
    safe fn BrotliEncoderMaxCompressedSize(input_size: usize) -> usize;
    fn BrotliEncoderCompress(
        quality: c_int,
        lgwin: c_int,
        mode: c_int,
        input_size: usize,
        input_buffer: *const u8,
        encoded_size: *mut usize,
        encoded_buffer: *mut u8,
    ) -> c_int;
    /// A new decoder state on malloc, or none. Its caller owns it until the unsafe
    /// `BrotliDecoderDestroyInstance`.
    safe fn BrotliDecoderCreateInstance(
        alloc: DefaultAllocator,
        free: DefaultAllocator,
        opaque: DefaultAllocator,
    ) -> Option<Owned>;
    fn BrotliDecoderDestroyInstance(state: *mut State);
    fn BrotliDecoderDecompressStream(
        state: *mut State,
        available_in: *mut usize,
        next_in: *mut *const u8,
        available_out: *mut usize,
        next_out: *mut *mut u8,
        total_out: *mut usize,
    ) -> c_int;
    /// The live state's last error: it reads the state and nothing else.
    safe fn BrotliDecoderGetErrorCode(state: Live<'_>) -> c_int;
    /// The code's name: a static string for every code, `INVALID` for an unknown one.
    safe fn BrotliDecoderErrorString(code: c_int) -> Text<'static>;
}

/// MINFO's `brotli` row: `E<BrotliEncoderVersion>/D<BrotliDecoderVersion>`.
#[must_use]
pub fn versions() -> String {
    format!("E{}/D{}", BrotliEncoderVersion(), BrotliDecoderVersion())
}

/// `BrotliEncoderCompress` of `input` at `quality`, with the default window and
/// mode, into `BrotliEncoderMaxCompressedSize` bytes: None when it fails.
#[must_use]
pub fn compress(input: &[u8], quality: i32) -> Option<Vec<u8>> {
    let mut out = vec![0; BrotliEncoderMaxCompressedSize(input.len())];
    let mut size = out.len();
    // SAFETY: `input` is readable for its length and `out` writable for `size` bytes.
    let compressed = unsafe {
        BrotliEncoderCompress(
            quality,
            DEFAULT_WINDOW,
            DEFAULT_MODE,
            input.len(),
            input.as_ptr(),
            &raw mut size,
            out.as_mut_ptr(),
        )
    };
    (compressed == BROTLI_TRUE).then(|| {
        out.truncate(size);
        out
    })
}

/// A response's Brotli decoder. `out` holds the bytes the last chunk produced.
pub struct Decoder {
    state: Owned,
    out: Vec<u8>,
}

impl Decoder {
    /// A decoder on libbrotli's default allocator.
    /// # Errors
    /// Upstream's warning when libbrotli can't allocate one.
    pub fn new() -> Result<Self, String> {
        let state = BrotliDecoderCreateInstance(None, None, None)
            .ok_or("BrotliDecoderCreateInstance() failed")?;
        Ok(Self {
            state,
            out: Vec::new(),
        })
    }

    /// Decode one body chunk: the bytes it produced, growing the output until
    /// libbrotli has consumed the chunk or finished the stream.
    /// # Errors
    /// Upstream's warning, with libbrotli's reason, when the stream is corrupt.
    pub fn decode(&mut self, input: &[u8]) -> Result<&[u8], String> {
        self.out.clear();
        let (mut next_in, mut available_in) = (input.as_ptr(), input.len());
        loop {
            self.out.reserve(BUFFER);
            let spare = self.out.spare_capacity_mut();
            let (mut next_out, room) = (spare.as_mut_ptr().cast::<u8>(), spare.len());
            let mut available_out = room;
            // SAFETY: the decoder's live state; `next_in` has `available_in` readable
            // bytes left of `input` and `next_out` `available_out` writable bytes of
            // `out`'s spare capacity. libbrotli accepts a null `total_out`.
            let result = unsafe {
                BrotliDecoderDecompressStream(
                    self.state.as_ptr(),
                    &raw mut available_in,
                    &raw mut next_in,
                    &raw mut available_out,
                    &raw mut next_out,
                    null_mut(),
                )
            };
            let written = self.out.len() + room - available_out;
            // SAFETY: libbrotli initialised the `room - available_out` bytes it wrote.
            unsafe { self.out.set_len(written) };
            match result {
                SUCCESS | NEEDS_MORE_INPUT => return Ok(&self.out),
                NEEDS_MORE_OUTPUT => {}
                _ => return Err(self.error()),
            }
        }
    }

    /// `BrotliDecoderDecompressStream() failed, <BrotliDecoderErrorString>`.
    fn error(&self) -> String {
        let code = BrotliDecoderGetErrorCode(self.state.live());
        let reason = BrotliDecoderErrorString(code).get();
        format!(
            "BrotliDecoderDecompressStream() failed, {}",
            reason.to_string_lossy()
        )
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: the state `new` created, destroyed once.
        unsafe { BrotliDecoderDestroyInstance(self.state.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_in_chunks() {
        let payload = b"brotli body, compressed once and decoded in pieces. ".repeat(5000);
        let compressed = compress(&payload, MAX_QUALITY).unwrap();
        assert!(compressed.len() < payload.len());
        let mut decoder = Decoder::new().unwrap();
        let mut got = Vec::new();
        for piece in compressed.chunks(5) {
            got.extend_from_slice(decoder.decode(piece).unwrap());
        }
        assert_eq!(got, payload);
        // One chunk whose output exceeds the decoder's buffer.
        let mut decoder = Decoder::new().unwrap();
        assert_eq!(decoder.decode(&compressed).unwrap(), &payload[..]);
    }

    #[test]
    fn empty_input_compresses() {
        let compressed = compress(b"", MIN_QUALITY).unwrap();
        assert_eq!(Decoder::new().unwrap().decode(&compressed).unwrap(), []);
    }

    #[test]
    fn garbage_is_an_error() {
        let error = Decoder::new().unwrap().decode(&[0xff; 64]).unwrap_err();
        assert!(
            error.starts_with("BrotliDecoderDecompressStream() failed, _ERROR_"),
            "{error}"
        );
    }
}
