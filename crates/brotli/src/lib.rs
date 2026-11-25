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
use std::ptr::NonNull;
use swoole_rs_corebase::lent::{Count, In, Out, lend_in, lend_out};
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
/// A decoder state `BrotliDecoderCreateInstance` made, which its drop destroys. It is
/// neither `Copy` nor `Clone`, so it is destroyed once, and not `Send`, as the pointer
/// it replaced was not.
#[repr(transparent)]
struct Owned(NonNull<State>);

impl Owned {
    /// The state while its owner is borrowed, for the calls that use it.
    const fn live(&self) -> Live<'_> {
        Live(self.0, PhantomData)
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        BrotliDecoderDestroyInstance(Dying(self.0));
    }
}

/// An [`Owned`] state, borrowed: what a libbrotli call that uses the state takes.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Live<'a>(NonNull<State>, PhantomData<&'a Owned>);
/// An [`Owned`] state that its drop alone makes, for the call that frees it.
#[repr(transparent)]
struct Dying(NonNull<State>);
/// Where libbrotli takes a custom allocator's `alloc`, `free` or `opaque`: always
/// null here (nothing makes the reference), which selects malloc and free.
type DefaultAllocator = Option<&'static !>;

// libbrotli's own declarations. Each reads the lent input, writes at most the lent
// room, moves a lending's address and count on together, and keeps no argument; the
// decoder calls read or write only the live state they are given, and the destroy
// call frees the state no one holds afterwards.
unsafe extern "C" {
    safe fn BrotliEncoderVersion() -> u32;
    safe fn BrotliDecoderVersion() -> u32;
    safe fn BrotliEncoderMaxCompressedSize(input_size: usize) -> usize;
    safe fn BrotliEncoderCompress<'i, 'o>(
        quality: c_int,
        lgwin: c_int,
        mode: c_int,
        input_size: Count<'i>,
        input_buffer: In<'i, u8>,
        encoded_size: &mut Count<'o>,
        encoded_buffer: Out<'o, u8>,
    ) -> c_int;
    /// A new decoder state on malloc, or none.
    safe fn BrotliDecoderCreateInstance(
        alloc: DefaultAllocator,
        free: DefaultAllocator,
        opaque: DefaultAllocator,
    ) -> Option<Owned>;
    safe fn BrotliDecoderDestroyInstance(state: Dying);
    /// Accepts a null `total_out`.
    safe fn BrotliDecoderDecompressStream<'i, 'o>(
        state: Live<'_>,
        available_in: &mut Count<'i>,
        next_in: &mut In<'i, u8>,
        available_out: &mut Count<'o>,
        next_out: &mut Out<'o, u8>,
        total_out: Option<&mut usize>,
    ) -> c_int;
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
    let size = lend_in(input, |input, size| {
        lend_out(&mut out, |out, mut room| {
            let compressed = BrotliEncoderCompress(
                quality,
                DEFAULT_WINDOW,
                DEFAULT_MODE,
                size,
                input,
                &mut room,
                out,
            );
            (compressed == BROTLI_TRUE).then_some(room.get())
        })
    })?;
    out.truncate(size);
    Some(out)
}

/// Why a response chunk could not be decoded.
#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The response exceeded its configured decoded byte limit.
    Limit(usize),
    /// The codec's diagnostic for malformed input.
    Codec(String),
}

/// A response's Brotli decoder. The first bytes of `out` hold what the last chunk
/// produced; all of it is initialised, so it grows, zeroed, only past its largest.
pub struct Decoder {
    state: Owned,
    out: Vec<u8>,
    limit: usize,
    remaining: usize,
}

impl Decoder {
    /// A decoder on libbrotli's default allocator.
    /// # Errors
    /// Upstream's warning when libbrotli can't allocate one.
    pub fn new(limit: usize) -> Result<Self, String> {
        let state = BrotliDecoderCreateInstance(None, None, None)
            .ok_or("BrotliDecoderCreateInstance() failed")?;
        Ok(Self {
            state,
            out: Vec::new(),
            limit,
            remaining: limit,
        })
    }

    /// Decode one body chunk: the bytes it produced, growing the output until
    /// libbrotli has consumed the chunk or finished the stream.
    /// # Errors
    /// Returns the response limit on overflow or libbrotli's diagnostic on corrupt input.
    pub fn decode(&mut self, input: &[u8]) -> Result<&[u8], DecodeError> {
        let (state, out) = (self.state.live(), &mut self.out);
        let decoded = lend_in(input, |mut next_in, mut available_in| {
            let mut filled = 0_usize;
            loop {
                // One extra byte distinguishes an exact fit from excess output.
                let end = filled
                    .saturating_add(BUFFER)
                    .min(self.remaining.saturating_add(1));
                if out.len() < end {
                    out.resize(end, 0);
                }
                let room = out.get_mut(filled..end).unwrap_or_default();
                let size = room.len();
                let (result, left) = lend_out(room, |mut next_out, mut available_out| {
                    let result = BrotliDecoderDecompressStream(
                        state,
                        &mut available_in,
                        &mut next_in,
                        &mut available_out,
                        &mut next_out,
                        None,
                    );
                    (result, available_out.get())
                });
                filled += size - left;
                if filled > self.remaining {
                    return Err(DecodeError::Limit(self.limit));
                }
                match result {
                    SUCCESS | NEEDS_MORE_INPUT => return Ok(filled),
                    NEEDS_MORE_OUTPUT => {}
                    _ => return Err(DecodeError::Codec(error(state))),
                }
            }
        })?;
        self.remaining -= decoded;
        Ok(self.out.get(..decoded).unwrap_or_default())
    }
}

/// `BrotliDecoderDecompressStream() failed, <BrotliDecoderErrorString>`.
fn error(state: Live<'_>) -> String {
    let reason = BrotliDecoderErrorString(BrotliDecoderGetErrorCode(state)).get();
    format!(
        "BrotliDecoderDecompressStream() failed, {}",
        reason.to_string_lossy()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_in_chunks() {
        let payload = b"brotli body, compressed once and decoded in pieces. ".repeat(5000);
        let compressed = compress(&payload, MAX_QUALITY).unwrap();
        assert!(compressed.len() < payload.len());
        let mut decoder = Decoder::new(payload.len()).unwrap();
        let mut got = Vec::new();
        for piece in compressed.chunks(5) {
            got.extend_from_slice(decoder.decode(piece).unwrap());
        }
        assert_eq!(got, payload);
        let mut bounded = Decoder::new(payload.len() - 1).unwrap();
        assert_eq!(
            compressed
                .chunks(5)
                .try_for_each(|chunk| bounded.decode(chunk).map(|_| ())),
            Err(DecodeError::Limit(payload.len() - 1))
        );
        // One chunk whose output exceeds the decoder's buffer.
        let mut decoder = Decoder::new(payload.len()).unwrap();
        assert_eq!(decoder.decode(&compressed).unwrap(), &payload[..]);
    }

    #[test]
    fn empty_input_compresses() {
        let compressed = compress(b"", MIN_QUALITY).unwrap();
        assert_eq!(Decoder::new(0).unwrap().decode(&compressed).unwrap(), []);
    }

    #[test]
    fn garbage_is_an_error() {
        let error = Decoder::new(0).unwrap().decode(&[0xff; 64]).unwrap_err();
        let DecodeError::Codec(error) = error else {
            panic!("expected a codec error")
        };
        assert!(
            error.starts_with("BrotliDecoderDecompressStream() failed, _ERROR_"),
            "{error}"
        );
    }
}
