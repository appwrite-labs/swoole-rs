//! zstd over the system libzstd: the server's one-shot `ZSTD_compress`
//! (`HttpContext::compress`) and the client's streaming decoder
//! (`Client::decompress_response`).

use ::zstd::stream::raw::{Decoder as Stream, InBuffer, Operation, OutBuffer};

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
    out: Vec<u8>,
}

impl Decoder {
    /// A started decompression stream.
    /// # Errors
    /// Upstream's warning when the stream cannot be started.
    pub fn new() -> Result<Self, String> {
        let stream = Stream::new()
            .map_err(|error| format!("ZSTD_initDStream() failed, Error: [{error}]"))?;
        Ok(Self {
            stream,
            out: Vec::new(),
        })
    }

    /// Decode one body chunk: the bytes it produced, flushed until the decoder
    /// has no input left and room left to write.
    /// # Errors
    /// Upstream's warning when the stream is corrupt.
    pub fn decode(&mut self, input: &[u8]) -> Result<&[u8], String> {
        self.out.clear();
        let mut source = InBuffer::around(input);
        loop {
            if self.out.len() == self.out.capacity() {
                self.out.reserve(::zstd::zstd_safe::DCtx::out_size());
            }
            let start = self.out.len();
            let mut sink = OutBuffer::around_pos(&mut self.out, start);
            self.stream
                .run(&mut source, &mut sink)
                .map_err(|error| format!("ZSTD_decompressStream() failed, Error: [{error}]"))?;
            if source.pos() == input.len() && self.out.len() < self.out.capacity() {
                return Ok(&self.out);
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
        let mut decoder = Decoder::new().unwrap();
        let mut got = Vec::new();
        for piece in compressed.chunks(3) {
            got.extend_from_slice(decoder.decode(piece).unwrap());
        }
        assert_eq!(got, payload);
        // One chunk whose output exceeds a buffer's worth.
        let mut decoder = Decoder::new().unwrap();
        assert_eq!(decoder.decode(&compressed).unwrap(), &payload[..]);
    }

    #[test]
    fn garbage_is_an_error() {
        let error = Decoder::new().unwrap().decode(&[0xff; 64]).unwrap_err();
        assert!(
            error.starts_with("ZSTD_decompressStream() failed, Error: ["),
            "{error}"
        );
    }
}
