//! crates/http2's HPACK codec: RFC 7541 Appendix C's examples, nghttp2's
//! encoder choices and table size updates, its decoding errors, and a
//! differential run against the nghttp2 inflater upstream bundles
//! (crates/oracle's copy).
#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test helpers outside #[test] fns; a panic fails the case"
)]
use swoole_rs_http2::hpack::{Decoder, Encoder, Error};
use swoole_rs_oracle::nghttp2;

type Headers = Vec<(Vec<u8>, Vec<u8>)>;

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

fn list(headers: &[(&str, &str)]) -> Headers {
    headers
        .iter()
        .map(|(name, value)| (name.as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect()
}

/// A whole block, in fragments of `split` bytes, the last one final.
fn decode(decoder: &mut Decoder, block: &[u8], split: usize) -> Result<Headers, Error> {
    let mut headers = Vec::new();
    let fragments: Vec<&[u8]> = block.chunks(split.max(1)).collect();
    let count = fragments.len().max(1);
    for at in 0..count {
        let mut input = fragments.get(at).copied().unwrap_or_default();
        let last = at + 1 == count;
        loop {
            let step = decoder.decode(input, last)?;
            input = &input[step.read..];
            if let Some((name, value)) = step.header {
                headers.push((name.to_vec(), value.to_vec()));
            }
            if step.done {
                decoder.end_headers();
                return Ok(headers);
            }
            if step.header.is_none() && input.is_empty() {
                break;
            }
        }
    }
    unreachable!("a final fragment always ends the block")
}

fn encode(encoder: &mut Encoder, headers: &Headers) -> Vec<u8> {
    let mut out = Vec::new();
    encoder.encode(
        headers
            .iter()
            .map(|(name, value)| (name.as_slice(), value.as_slice())),
        &mut out,
    );
    out
}

/// One example: its encoding, its header list and the table size after it.
type Example<'a> = (&'a str, &'a [(&'a str, &'a str)], usize);

const REQUESTS: [&[(&str, &str)]; 3] = [
    &[
        (":method", "GET"),
        (":scheme", "http"),
        (":path", "/"),
        (":authority", "www.example.com"),
    ],
    &[
        (":method", "GET"),
        (":scheme", "http"),
        (":path", "/"),
        (":authority", "www.example.com"),
        ("cache-control", "no-cache"),
    ],
    &[
        (":method", "GET"),
        (":scheme", "https"),
        (":path", "/index.html"),
        (":authority", "www.example.com"),
        ("custom-key", "custom-value"),
    ],
];

const RESPONSES: [&[(&str, &str)]; 3] = [
    &[
        (":status", "302"),
        ("cache-control", "private"),
        ("date", "Mon, 21 Oct 2013 20:13:21 GMT"),
        ("location", "https://www.example.com"),
    ],
    &[
        (":status", "307"),
        ("cache-control", "private"),
        ("date", "Mon, 21 Oct 2013 20:13:21 GMT"),
        ("location", "https://www.example.com"),
    ],
    &[
        (":status", "200"),
        ("cache-control", "private"),
        ("date", "Mon, 21 Oct 2013 20:13:22 GMT"),
        ("location", "https://www.example.com"),
        ("content-encoding", "gzip"),
        (
            "set-cookie",
            "foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1",
        ),
    ],
];

/// C.3: requests without Huffman coding.
const C3: [Example; 3] = [
    ("828684410f7777772e6578616d706c652e636f6d", REQUESTS[0], 57),
    ("828684be58086e6f2d6361636865", REQUESTS[1], 110),
    (
        "828785bf400a637573746f6d2d6b65790c637573746f6d2d76616c7565",
        REQUESTS[2],
        164,
    ),
];

/// C.4: the same requests with Huffman coding.
const C4: [Example; 3] = [
    ("828684418cf1e3c2e5f23a6ba0ab90f4ff", REQUESTS[0], 57),
    ("828684be5886a8eb10649cbf", REQUESTS[1], 110),
    (
        "828785bf408825a849e95ba97d7f8925a849e95bb8e8b4bf",
        REQUESTS[2],
        164,
    ),
];

/// C.5: responses without Huffman coding, in a 256-byte table.
const C5: [Example; 3] = [
    (
        "4803333032580770726976617465611d4d6f6e2c203231204f637420323031332032303a31333a323120474d546e1768747470733a2f2f7777772e6578616d706c652e636f6d",
        RESPONSES[0],
        222,
    ),
    ("4803333037c1c0bf", RESPONSES[1], 222),
    (
        "88c1611d4d6f6e2c203231204f637420323031332032303a31333a323220474d54c05a04677a69707738666f6f3d4153444a4b48514b425a584f5157454f50495541585157454f49553b206d61782d6167653d333630303b2076657273696f6e3d31",
        RESPONSES[2],
        215,
    ),
];

/// C.6: the same responses with Huffman coding.
const C6: [Example; 3] = [
    (
        "488264025885aec3771a4b6196d07abe941054d444a8200595040b8166e082a62d1bff6e919d29ad171863c78f0b97c8e9ae82ae43d3",
        RESPONSES[0],
        222,
    ),
    ("4883640effc1c0bf", RESPONSES[1], 222),
    (
        "88c16196d07abe941054d444a8200595040b8166e084a62d1bffc05a839bd9ab77ad94e7821dd7f2e6c7b335dfdfcd5b3960d5af27087f3672c1ab270fb5291f9587316065c003ed4ee5b1063d5007",
        RESPONSES[2],
        215,
    ),
];

/// A table size update to 256, which C.5 and C.6 assume.
const SIZE_256: &str = "3fe101";

fn decode_examples(examples: &[Example], prefix: &str) {
    for split in [usize::MAX, 1, 2, 3, 7] {
        let mut decoder = Decoder::default();
        for (at, &(block, headers, size)) in examples.iter().enumerate() {
            let block = hex(&format!("{}{block}", if at == 0 { prefix } else { "" }));
            let decoded = decode(&mut decoder, &block, split).unwrap();
            assert_eq!(decoded, list(headers), "block {at}, fragments of {split}");
            assert_eq!(decoder.table_size(), size, "table after block {at}");
        }
    }
}

/// C.1.1 and C.1.2: 10 and 1337 with a 5-bit prefix, as the table size updates
/// a smaller encoder opens with. (C.1.3's 8-bit prefix is never used by HPACK
/// fields.)
#[test]
fn integers() {
    assert_eq!(encode(&mut Encoder::new(10), &Vec::new()), hex("2a"));
    assert_eq!(encode(&mut Encoder::new(1337), &Vec::new()), hex("3f9a0a"));
    let block = hex("3f9a0a");
    assert_eq!(decode(&mut Decoder::default(), &block, 1), Ok(Vec::new()));
}

/// C.2: one field of each representation.
#[test]
fn fields() {
    for (block, header, size) in [
        (
            "400a637573746f6d2d6b65790d637573746f6d2d686561646572",
            ("custom-key", "custom-header"),
            55,
        ),
        ("040c2f73616d706c652f70617468", (":path", "/sample/path"), 0),
        (
            "100870617373776f726406736563726574",
            ("password", "secret"),
            0,
        ),
        ("82", (":method", "GET"), 0),
    ] {
        let mut decoder = Decoder::default();
        let decoded = decode(&mut decoder, &hex(block), usize::MAX).unwrap();
        assert_eq!(decoded, list(&[header]), "{block}");
        assert_eq!(decoder.table_size(), size, "{block}");
    }
}

#[test]
fn requests() {
    decode_examples(&C3, "");
}

#[test]
fn requests_huffman() {
    decode_examples(&C4, "");
}

#[test]
fn responses() {
    decode_examples(&C5, SIZE_256);
}

#[test]
fn responses_huffman() {
    decode_examples(&C6, SIZE_256);
}

/// nghttp2's encoder codes C.4's requests exactly: it indexes the same fields
/// and Huffman-codes every literal, being shorter.
#[test]
fn encode_requests() {
    let mut encoder = Encoder::new(4096);
    for (at, &(block, headers, size)) in C4.iter().enumerate() {
        assert_eq!(
            encode(&mut encoder, &list(headers)),
            hex(block),
            "request {at}"
        );
        assert_eq!(encoder.table_size(), size, "table after request {at}");
    }
}

/// C.6's responses differ: nghttp2 never indexes `location` or `set-cookie`, and
/// opens with the 256-byte table's size update. Each decodes as it was given,
/// with both tables in step.
#[test]
fn encode_responses() {
    let (mut encoder, mut decoder) = (Encoder::new(256), Decoder::default());
    for (at, &(_, headers, _)) in C6.iter().enumerate() {
        let block = encode(&mut encoder, &list(headers));
        assert_eq!(block.starts_with(&hex(SIZE_256)), at == 0, "response {at}");
        let decoded = decode(&mut decoder, &block, usize::MAX).unwrap();
        assert_eq!(decoded, list(headers), "response {at}");
        assert_eq!(
            decoder.table_size(),
            encoder.table_size(),
            "table after {at}"
        );
    }
}

/// Credentials and short cookies are never indexed; fields too big for three
/// quarters of the table are not indexed.
#[test]
fn encode_indexing() {
    let mut encoder = Encoder::new(4096);
    let headers = list(&[
        ("authorization", "secret"),
        ("cookie", "short"),
        ("x-big", &"a".repeat(3100)),
        (":path", "/a"),
        ("content-length", "1"),
    ]);
    let block = encode(&mut encoder, &headers);
    assert_eq!(encoder.table_size(), 0, "nothing indexed");
    assert_eq!(block[0], 0x1f, "authorization never indexed, by name 23");
    let mut decoder = Decoder::default();
    assert_eq!(decode(&mut decoder, &block, 5), Ok(headers.clone()));
    let again = encode(&mut encoder, &headers);
    assert_eq!(again, block, "nothing to reuse");
}

/// A shrink then a grow between blocks sends both sizes; a peer's size is capped
/// at the encoder's own limit.
#[test]
fn encode_table_size() {
    let mut encoder = Encoder::new(4096);
    let headers = list(REQUESTS[2]);
    encode(&mut encoder, &headers);
    encoder.set_table_size(0);
    assert_eq!(encoder.table_size(), 0, "evicted");
    encoder.set_table_size(8192);
    let block = encode(&mut encoder, &Vec::new());
    assert_eq!(block, hex("203fe11f"), "0, then 4096");
    let block = encode(&mut encoder, &Vec::new());
    assert!(block.is_empty(), "sent once");
    encoder.set_table_size(100);
    let mut decoder = Decoder::default();
    let block = encode(&mut encoder, &headers);
    assert_eq!(block[..2], hex("3f45"), "100");
    assert_eq!(decode(&mut decoder, &block, usize::MAX), Ok(headers));
    assert_eq!(decoder.table_size(), encoder.table_size(), "in step");
}

#[test]
fn decode_errors() {
    for (block, why) in [
        ("80", "index 0"),
        ("be", "index past the table"),
        ("8220", "table size update after a field"),
        ("3fe21f", "table size over 4096"),
        ("4084ffffffff0161", "Huffman EOS"),
        ("40821fff0161", "Huffman padding over seven bits"),
        ("40811e0161", "Huffman padding not all ones"),
        ("4f", "value cut short"),
        ("7f", "index cut short"),
        ("4003616263", "value missing"),
        ("ffffffffffff0f", "integer overflow"),
        ("ff80808080808000", "integer too long"),
        ("407f818004", "name longer than 65536"),
    ] {
        let mut decoder = Decoder::default();
        assert_eq!(
            decode(&mut decoder, &hex(block), usize::MAX),
            Err(Error),
            "{why}"
        );
        assert_eq!(
            decoder.decode(&hex("82"), true),
            Err(Error),
            "{why}: stays bad"
        );
    }
    assert_eq!(Error::CODE, -523, "NGHTTP2_ERR_HEADER_COMP");
    assert_eq!(Error.to_string(), "Header compression/decompression error");
}

/// The step contract callers loop on: a field is returned with what it took,
/// and the block ends on an empty final call.
#[test]
fn steps() {
    let mut decoder = Decoder::default();
    let block = hex("8286");
    let step = decoder.decode(&block, true).unwrap();
    assert_eq!((step.read, step.flags()), (1, 2), "first field");
    let step = decoder.decode(&block[1..], true).unwrap();
    assert_eq!((step.read, step.flags()), (1, 2), "second field");
    let step = decoder.decode(&[], true).unwrap();
    assert_eq!(
        (step.read, step.flags(), step.header),
        (0, 1, None),
        "final"
    );
    let step = decoder.decode(&hex("40"), false).unwrap();
    assert_eq!(
        (step.read, step.flags()),
        (1, 0),
        "a fragment's partial field"
    );
    assert_eq!(
        decoder.decode(&[], true),
        Err(Error),
        "cut short at the end"
    );
}

/// The client's loop (`sw_rs_http2_inflate_step` in the client role) stops once
/// a final block's input is consumed, even on a field, before any `done` step:
/// unless it then ends the block, the next block's opening table size update
/// is refused, as nghttp2 refuses it.
#[test]
fn client_block_then_table_size_update() {
    let client_block = |decoder: &mut Decoder, block: &[u8], end: bool| {
        let mut input = block;
        loop {
            let step = decoder.decode(input, true)?;
            input = &input[step.read..];
            let plan = swoole_rs_http2::sw_rs_http2_inflate_step(step.flags(), input.len(), 1);
            if plan.end_headers != 0 || plan.finish != 0 {
                if end {
                    decoder.end_headers();
                }
                return Ok(());
            }
        }
    };
    let (first, second) = (hex("82"), hex("3fe1018286"));
    let mut decoder = Decoder::default();
    client_block(&mut decoder, &first, false).unwrap();
    assert_eq!(
        client_block(&mut decoder, &second, false),
        Err(Error),
        "still in block"
    );
    let mut decoder = Decoder::default();
    client_block(&mut decoder, &first, true).unwrap();
    assert_eq!(
        client_block(&mut decoder, &second, true),
        Ok(()),
        "a new block"
    );
}

/// A small deterministic generator.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.0 >> 33
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next()).unwrap() % bound
    }
}

/// What a step loop saw: each field, then whether the block ended or failed.
fn oracle(inflater: *mut core::ffi::c_void, block: &[u8], split: usize) -> (Headers, bool) {
    let mut headers = Vec::new();
    let fragments: Vec<&[u8]> = block.chunks(split.max(1)).collect();
    let count = fragments.len().max(1);
    for at in 0..count {
        let mut input = fragments.get(at).copied().unwrap_or_default();
        let last = at + 1 == count;
        loop {
            let mut nv = nghttp2::Nv {
                name: std::ptr::null(),
                value: std::ptr::null(),
                name_length: 0,
                value_length: 0,
                flags: 0,
            };
            let mut flags = 0;
            // SAFETY: a live inflater over `input`; `nv` and `flags` are writable.
            let read = unsafe {
                nghttp2::nghttp2_hd_inflate_hd(
                    inflater,
                    &raw mut nv,
                    &raw mut flags,
                    input.as_ptr(),
                    input.len(),
                    i32::from(last),
                )
            };
            let Ok(read) = usize::try_from(read) else {
                return (headers, false);
            };
            input = &input[read..];
            if flags & nghttp2::INFLATE_EMIT != 0 {
                // SAFETY: nghttp2's field, live until the next call.
                let name = unsafe { std::slice::from_raw_parts(nv.name, nv.name_length) };
                // SAFETY: as above.
                let value = unsafe { std::slice::from_raw_parts(nv.value, nv.value_length) };
                headers.push((name.to_vec(), value.to_vec()));
            }
            if flags & nghttp2::INFLATE_FINAL != 0 {
                // SAFETY: a live inflater.
                unsafe { nghttp2::nghttp2_hd_inflate_end_headers(inflater) };
                return (headers, true);
            }
            if flags & nghttp2::INFLATE_EMIT == 0 && input.is_empty() {
                break;
            }
        }
    }
    unreachable!("a final fragment always ends the block")
}

fn rust(decoder: &mut Decoder, block: &[u8], split: usize) -> (Headers, bool) {
    let mut headers = Vec::new();
    let fragments: Vec<&[u8]> = block.chunks(split.max(1)).collect();
    let count = fragments.len().max(1);
    for at in 0..count {
        let mut input = fragments.get(at).copied().unwrap_or_default();
        let last = at + 1 == count;
        loop {
            let Ok(step) = decoder.decode(input, last) else {
                return (headers, false);
            };
            input = &input[step.read..];
            if let Some((name, value)) = step.header {
                headers.push((name.to_vec(), value.to_vec()));
            }
            if step.done {
                decoder.end_headers();
                return (headers, true);
            }
            if step.header.is_none() && input.is_empty() {
                break;
            }
        }
    }
    unreachable!("a final fragment always ends the block")
}

/// Encoded blocks, then the same with bytes changed, decoded block after block
/// on one connection by both inflaters in random fragments: the same fields,
/// and the same blocks fail.
#[test]
fn matches_nghttp2() {
    const NAMES: [&str; 8] = [
        ":status",
        "content-type",
        "x-custom",
        "set-cookie",
        "cookie",
        "date",
        "authorization",
        "x-a-much-longer-header-name",
    ];
    let mut random = Random(7541);
    for connection in 0..400 {
        let mut encoder = Encoder::new(if connection % 3 == 0 { 256 } else { 4096 });
        let mut decoder = Decoder::default();
        let mut inflater = std::ptr::null_mut();
        // SAFETY: `inflater` is writable.
        assert_eq!(
            unsafe { nghttp2::nghttp2_hd_inflate_new(&raw mut inflater) },
            0
        );
        // A changed byte that still decodes leaves the tables out of step.
        let mut mutated = false;
        for _ in 0..8 {
            let headers: Headers = (0..random.below(6))
                .map(|_| {
                    let name = NAMES[random.below(NAMES.len())].as_bytes().to_vec();
                    let value = (0..random.below(40))
                        .map(|_| u8::try_from(random.next() & 0xff).unwrap())
                        .collect();
                    (name, value)
                })
                .collect();
            if random.below(5) == 0 {
                encoder.set_table_size(u32::try_from(random.below(5000)).unwrap());
            }
            let mut block = encode(&mut encoder, &headers);
            if connection % 2 == 1 && !block.is_empty() && random.below(3) == 0 {
                let at = random.below(block.len());
                block[at] = u8::try_from(random.next() & 0xff).unwrap();
                mutated = true;
            }
            let split = 1 + random.below(block.len().max(1));
            let expected = oracle(inflater, &block, split);
            let actual = rust(&mut decoder, &block, split);
            assert_eq!(actual, expected, "{block:x?} in fragments of {split}");
            if !mutated {
                assert_eq!(actual, (headers, true), "round trip");
            }
            if !expected.1 {
                break;
            }
        }
        // SAFETY: the inflater made above.
        unsafe { nghttp2::nghttp2_hd_inflate_del(inflater) };
    }
}
