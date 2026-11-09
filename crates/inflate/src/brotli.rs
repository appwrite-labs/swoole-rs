//! Brotli: the server's one-shot compression (`HttpContext::compress`) and the
//! client's streaming decoder (`Client::decompress_response`).

use std::io::Write;

/// `BROTLI_MIN_QUALITY`, `BROTLI_MAX_QUALITY`.
pub const MIN_QUALITY: i32 = 0;
pub const MAX_QUALITY: i32 = 11;
/// The decoder's output buffer.
const BUFFER: usize = 16384;

/// `BrotliEncoderCompress` of `input` at `quality`, with the default window
/// (`BROTLI_DEFAULT_WINDOW`, 22) and mode: None when it fails.
#[must_use]
pub fn compress(input: &[u8], quality: i32) -> Option<Vec<u8>> {
    let params = ::brotli::enc::BrotliEncoderParams {
        quality,
        ..Default::default()
    };
    let mut out = Vec::new();
    ::brotli::BrotliCompress(&mut &input[..], &mut out, &params).ok()?;
    Some(out)
}

/// A response's Brotli decoder.
pub struct Decoder {
    writer: ::brotli::DecompressorWriter<Vec<u8>>,
    out: Vec<u8>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            writer: ::brotli::DecompressorWriter::new(Vec::new(), BUFFER),
            out: Vec::new(),
        }
    }
}

impl Decoder {
    /// Decode one body chunk: the bytes it produced.
    /// # Errors
    /// Upstream's warning when the stream is corrupt.
    pub fn decode(&mut self, input: &[u8]) -> Result<&[u8], String> {
        self.writer
            .write_all(input)
            .map_err(|error| format!("BrotliDecoderDecompressStream() failed, {error}"))?;
        self.out = std::mem::take(self.writer.get_mut());
        Ok(&self.out)
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
        let mut decoder = Decoder::default();
        let mut got = Vec::new();
        for piece in compressed.chunks(5) {
            got.extend_from_slice(decoder.decode(piece).unwrap());
        }
        assert_eq!(got, payload);
        // One chunk whose output exceeds the decoder's buffer.
        let mut decoder = Decoder::default();
        assert_eq!(decoder.decode(&compressed).unwrap(), &payload[..]);
    }

    #[test]
    fn garbage_is_an_error() {
        let error = Decoder::default().decode(&[0xff; 64]).unwrap_err();
        assert!(
            error.starts_with("BrotliDecoderDecompressStream() failed, "),
            "{error}"
        );
    }
}
