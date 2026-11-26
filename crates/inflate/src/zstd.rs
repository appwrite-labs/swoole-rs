//! zstd over the system libzstd: the server's one-shot `ZSTD_compress`
//! (`HttpContext::compress`) and the client's streaming decoder
//! (`Client::decompress_response`).

use crate::DecodeError;
use ::zstd::stream::raw::{Decoder as Stream, InBuffer, Operation, OutBuffer};
use swoole_rs_corebase::buffer::BoundedBytes;

/// `ZSTD_minCLevel()` and `ZSTD_maxCLevel()`.
#[must_use]
pub fn levels() -> (i32, i32) {
    let range = ::zstd::compression_level_range();
    (*range.start(), *range.end())
}

/// `ZSTD_compress` of `input` at `level`.
/// # Errors
/// Upstream's warning when compression fails.
pub fn compress(input: &[u8], level: i32) -> Result<Vec<u8>, String> {
    ::zstd::bulk::compress(input, level)
        .map_err(|error| format!("ZSTD_compress() failed, Error: [{error}]"))
}

/// A response's zstd decoder. `out` holds the bytes the last chunk produced.
pub struct Decoder {
    stream: Stream<'static>,
    out: BoundedBytes,
    limit: usize,
    remaining: usize,
}

impl Decoder {
    /// A started decompression stream.
    /// # Errors
    /// Upstream's warning when the stream cannot be started.
    pub fn new(limit: usize) -> Result<Self, String> {
        let stream = Stream::new()
            .map_err(|error| format!("ZSTD_initDStream() failed, Error: [{error}]"))?;
        Ok(Self {
            stream,
            out: BoundedBytes::new(limit.saturating_add(1), 0),
            limit,
            remaining: limit,
        })
    }

    /// Decode one body chunk: the bytes it produced, flushed until the decoder
    /// has no input left and room left to write.
    /// # Errors
    /// Returns the configured limit on overflow or zstd's diagnostic on corrupt input.
    pub fn decode(&mut self, input: &[u8]) -> Result<&[u8], DecodeError> {
        let mut source = InBuffer::around(input);
        let mut filled = 0_usize;
        loop {
            // Lend at most one byte past the remaining budget to detect overflow.
            let end = filled
                .saturating_add(::zstd::zstd_safe::DCtx::out_size())
                .min(self.remaining.saturating_add(1));
            let prefix = self
                .out
                .writable_prefix(end)
                .map_err(|_| DecodeError::Limit(self.limit))?;
            let room = prefix.get_mut(filled..).unwrap_or_default();
            let mut sink = OutBuffer::around(room);
            self.stream.run(&mut source, &mut sink).map_err(|error| {
                DecodeError::Codec(format!("ZSTD_decompressStream() failed, Error: [{error}]"))
            })?;
            filled += sink.pos();
            if filled > self.remaining {
                return Err(DecodeError::Limit(self.limit));
            }
            if source.pos() == input.len() && filled < end {
                self.remaining -= filled;
                return Ok(self.out.as_ref().get(..filled).unwrap_or_default());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_in_chunks() {
        let payload = b"zstd body, compressed once and decoded in pieces. ".repeat(20000);
        let (minimum, maximum) = levels();
        assert!(minimum < 0 && maximum >= 19);
        let compressed = compress(&payload, 20).unwrap();
        assert!(compressed.len() < payload.len());
        let mut decoder = Decoder::new(payload.len()).unwrap();
        let mut got = Vec::new();
        for piece in compressed.chunks(3) {
            got.extend_from_slice(decoder.decode(piece).unwrap());
        }
        assert_eq!(got, payload);
        // One chunk whose output exceeds a buffer's worth.
        let mut decoder = Decoder::new(payload.len()).unwrap();
        assert_eq!(decoder.decode(&compressed).unwrap(), &payload[..]);
    }

    #[test]
    fn response_limit_spans_frames_and_chunks() {
        let frame = compress(b"abc", 1).unwrap();
        let mut decoder = Decoder::new(5).unwrap();
        assert_eq!(decoder.decode(&frame).unwrap(), b"abc");
        assert_eq!(decoder.decode(&frame), Err(DecodeError::Limit(5)));
        let mut empty = Decoder::new(0).unwrap();
        assert_eq!(empty.decode(&compress(b"", 1).unwrap()).unwrap(), b"");
        assert_eq!(empty.decode(&frame), Err(DecodeError::Limit(0)));
    }

    #[test]
    fn garbage_is_an_error() {
        let error = Decoder::new(0).unwrap().decode(&[0xff; 64]).unwrap_err();
        let DecodeError::Codec(error) = error else {
            panic!("expected a codec error")
        };
        assert!(
            error.starts_with("ZSTD_decompressStream() failed, Error: ["),
            "{error}"
        );
    }
}
