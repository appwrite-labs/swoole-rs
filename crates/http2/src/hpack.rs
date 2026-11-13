//! HPACK (RFC 7541) for the HTTP/2 server and client, coded as the nghttp2
//! `nghttp2_hd` subset upstream bundles: the same indexing choices, table-size
//! updates, limits and errors, so either peer sees the bytes it saw before.

/// Bytes an entry costs beyond its name and value (RFC 7541 4.1).
const ENTRY_OVERHEAD: usize = 32;
/// `SETTINGS_HEADER_TABLE_SIZE`'s default, and the decoder's fixed limit.
const DEFAULT_TABLE_SIZE: usize = 4096;
/// nghttp2's `NGHTTP2_HD_MAX_NV`: the longest name or value it decodes.
const MAX_STRING: usize = 65536;

/// `NGHTTP2_ERR_HEADER_COMP`, the one error decoding reports. Once a decoder
/// fails, every later call fails too: its table is out of step with the peer's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error;

impl Error {
    /// nghttp2's code for it.
    pub const CODE: i32 = -523;
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Header compression/decompression error")
    }
}

/// RFC 7541 Appendix A.
static STATIC: [(&[u8], &[u8]); 61] = [
    (b":authority", b""),
    (b":method", b"GET"),
    (b":method", b"POST"),
    (b":path", b"/"),
    (b":path", b"/index.html"),
    (b":scheme", b"http"),
    (b":scheme", b"https"),
    (b":status", b"200"),
    (b":status", b"204"),
    (b":status", b"206"),
    (b":status", b"304"),
    (b":status", b"400"),
    (b":status", b"404"),
    (b":status", b"500"),
    (b"accept-charset", b""),
    (b"accept-encoding", b"gzip, deflate"),
    (b"accept-language", b""),
    (b"accept-ranges", b""),
    (b"accept", b""),
    (b"access-control-allow-origin", b""),
    (b"age", b""),
    (b"allow", b""),
    (b"authorization", b""),
    (b"cache-control", b""),
    (b"content-disposition", b""),
    (b"content-encoding", b""),
    (b"content-language", b""),
    (b"content-length", b""),
    (b"content-location", b""),
    (b"content-range", b""),
    (b"content-type", b""),
    (b"cookie", b""),
    (b"date", b""),
    (b"etag", b""),
    (b"expect", b""),
    (b"expires", b""),
    (b"from", b""),
    (b"host", b""),
    (b"if-match", b""),
    (b"if-modified-since", b""),
    (b"if-none-match", b""),
    (b"if-range", b""),
    (b"if-unmodified-since", b""),
    (b"last-modified", b""),
    (b"link", b""),
    (b"location", b""),
    (b"max-forwards", b""),
    (b"proxy-authenticate", b""),
    (b"proxy-authorization", b""),
    (b"range", b""),
    (b"referer", b""),
    (b"refresh", b""),
    (b"retry-after", b""),
    (b"server", b""),
    (b"set-cookie", b""),
    (b"strict-transport-security", b""),
    (b"transfer-encoding", b""),
    (b"user-agent", b""),
    (b"vary", b""),
    (b"via", b""),
    (b"www-authenticate", b""),
];

/// The Huffman code's bit length per symbol, 256 being EOS (RFC 7541
/// Appendix B). The code is canonical, so the lengths alone define it.
const HUFFMAN_LENGTHS: [u8; 257] = [
    13, 23, 28, 28, 28, 28, 28, 28, 28, 24, 30, 28, 28, 30, 28, 28, 28, 28, 28, 28, 28, 28, 30, 28,
    28, 28, 28, 28, 28, 28, 28, 28, 6, 10, 10, 12, 13, 6, 8, 11, 10, 10, 8, 11, 8, 6, 6, 6, 5, 5,
    5, 6, 6, 6, 6, 6, 6, 6, 7, 8, 15, 6, 12, 10, 13, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 8, 7, 8, 13, 19, 13, 14, 6, 15, 5, 6, 5, 6, 5, 6, 6, 6, 5, 7, 7, 6, 6,
    6, 5, 6, 7, 6, 5, 5, 6, 7, 7, 7, 7, 7, 15, 11, 14, 13, 28, 20, 22, 20, 20, 22, 22, 22, 23, 22,
    23, 23, 23, 23, 23, 24, 23, 24, 24, 22, 23, 24, 23, 23, 23, 23, 21, 22, 23, 22, 23, 23, 24, 22,
    21, 20, 22, 22, 23, 23, 21, 23, 22, 22, 24, 21, 22, 23, 23, 21, 21, 22, 21, 23, 22, 23, 23, 20,
    22, 22, 22, 23, 22, 22, 23, 26, 26, 20, 19, 22, 23, 22, 25, 26, 26, 26, 27, 27, 26, 24, 25, 19,
    21, 26, 27, 27, 26, 27, 24, 21, 21, 26, 26, 28, 27, 27, 27, 20, 24, 20, 21, 22, 21, 21, 23, 22,
    22, 25, 25, 24, 24, 26, 23, 26, 27, 26, 26, 27, 27, 27, 27, 27, 28, 27, 27, 27, 27, 27, 26, 30,
];
const MAX_CODE_LENGTH: usize = 30;

/// The canonical code: each symbol's code, and per length the first code,
/// how many codes have it and where they start in `symbols` (sorted by length,
/// then symbol).
struct Huffman {
    codes: [u32; 257],
    first: [u32; MAX_CODE_LENGTH + 1],
    count: [u32; MAX_CODE_LENGTH + 1],
    start: [u16; MAX_CODE_LENGTH + 1],
    symbols: [u16; 257],
}

#[expect(
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "const evaluation: lengths are 5..=30 and symbols < 257, checked at compile time"
)]
const HUFFMAN: Huffman = {
    let mut table = Huffman {
        codes: [0; 257],
        first: [0; MAX_CODE_LENGTH + 1],
        count: [0; MAX_CODE_LENGTH + 1],
        start: [0; MAX_CODE_LENGTH + 1],
        symbols: [0; 257],
    };
    let (mut code, mut next) = (0_u32, 0_usize);
    let mut length = 1;
    while length <= MAX_CODE_LENGTH {
        code <<= 1;
        table.first[length] = code;
        table.start[length] = next as u16;
        let mut symbol = 0;
        while symbol < 257 {
            if HUFFMAN_LENGTHS[symbol] as usize == length {
                table.codes[symbol] = code;
                table.symbols[next] = symbol as u16;
                code += 1;
                next += 1;
                table.count[length] += 1;
            }
            symbol += 1;
        }
        length += 1;
    }
    table
};

fn huffman_length(text: &[u8]) -> usize {
    let bits: usize = text
        .iter()
        .map(|&byte| usize::from(HUFFMAN_LENGTHS.get(usize::from(byte)).copied().unwrap_or(0)))
        .sum();
    bits.div_ceil(8)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "each byte pushed is the low eight bits of the shifted code"
)]
fn huffman_encode(text: &[u8], out: &mut Vec<u8>) {
    let (mut bits, mut pending) = (0_u64, 0_u32);
    for &byte in text {
        let symbol = usize::from(byte);
        let length = u32::from(HUFFMAN_LENGTHS.get(symbol).copied().unwrap_or(0));
        bits = (bits << length) | u64::from(HUFFMAN.codes.get(symbol).copied().unwrap_or(0));
        pending += length;
        while pending >= 8 {
            pending -= 8;
            out.push((bits >> pending) as u8);
        }
    }
    if pending > 0 {
        // EOS's prefix: ones.
        out.push((bits << (8 - pending)) as u8 | (0xff >> pending));
    }
}

/// Canonical decoding one bit at a time, resumable across fragments: the bits
/// of the symbol read so far.
#[derive(Clone, Copy, Default)]
struct Bits {
    code: u32,
    length: usize,
}

impl Bits {
    /// EOS fails as soon as its bytes arrive, as nghttp2's failure state does.
    fn decode(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
        for &byte in input {
            for shift in (0..8).rev() {
                self.code = (self.code << 1) | u32::from((byte >> shift) & 1);
                self.length += 1;
                let first = HUFFMAN.first.get(self.length).copied().ok_or(Error)?;
                let count = HUFFMAN.count.get(self.length).copied().ok_or(Error)?;
                let offset = self.code.wrapping_sub(first);
                if offset < count {
                    let at = usize::from(HUFFMAN.start.get(self.length).copied().ok_or(Error)?);
                    let symbol = HUFFMAN
                        .symbols
                        .get(at + offset as usize)
                        .copied()
                        .ok_or(Error)?;
                    // EOS (256) is no byte.
                    out.push(u8::try_from(symbol).map_err(|_| Error)?);
                    *self = Self::default();
                }
            }
        }
        Ok(())
    }

    /// The string ended: padding longer than seven bits or not all ones fails.
    fn finish(self) -> Result<(), Error> {
        if self.length > 7 || self.code != (1 << self.length) - 1 {
            return Err(Error);
        }
        Ok(())
    }
}

/// The first static entry named `name` (its value too if given), from 1.
fn static_index(name: &[u8], value: Option<&[u8]>) -> Option<usize> {
    let index = STATIC
        .iter()
        .position(|&(n, v)| n == name && value.is_none_or(|value| v == value))?;
    Some(index + 1)
}

/// A dynamic table entry's name and value.
type Entry = (Box<[u8]>, Box<[u8]>);

/// The dynamic table, oldest entry first.
struct Table {
    entries: Vec<Entry>,
    size: usize,
    max: usize,
}

impl Table {
    fn new(max: usize) -> Self {
        Self {
            entries: Vec::new(),
            size: 0,
            max,
        }
    }

    /// Index `index` (from 1) of the static then the dynamic table.
    fn get(&self, index: usize) -> Option<(&[u8], &[u8])> {
        let index = index.checked_sub(1)?;
        if let Some(&entry) = STATIC.get(index) {
            return Some(entry);
        }
        let newest = self.entries.len().checked_sub(1)?;
        let (name, value) = self
            .entries
            .get(newest.checked_sub(index - STATIC.len())?)?;
        Some((name, value))
    }

    fn len(&self) -> usize {
        STATIC.len() + self.entries.len()
    }

    /// Evict the oldest entries until `room` more bytes fit.
    fn evict(&mut self, room: usize) {
        let mut evicted = 0;
        for (name, value) in &self.entries {
            if self.size + room <= self.max {
                break;
            }
            self.size -= ENTRY_OVERHEAD + name.len() + value.len();
            evicted += 1;
        }
        self.entries.drain(..evicted);
    }

    fn resize(&mut self, max: usize) {
        self.max = max;
        self.evict(0);
    }

    /// An entry larger than the whole table empties it and is not kept.
    fn insert(&mut self, name: &[u8], value: &[u8]) {
        let room = ENTRY_OVERHEAD + name.len() + value.len();
        self.evict(room);
        if room <= self.max {
            self.entries.push((name.into(), value.into()));
            self.size += room;
        }
    }

    /// The newest dynamic entry named `name` (its value too if `exact`).
    fn find(&self, name: &[u8], value: Option<&[u8]>) -> Option<usize> {
        let newest = self
            .entries
            .iter()
            .rev()
            .position(|(n, v)| **n == *name && value.is_none_or(|value| **v == *value))?;
        Some(STATIC.len() + 1 + newest)
    }
}

/// An integer's continuation byte (RFC 7541 5.1) added in, with nghttp2's
/// bounds: 32 bits, at most five continuation bytes' worth of shift.
fn continue_integer(value: u32, shift: u32, byte: u8) -> Result<u32, Error> {
    let add = u32::from(byte & 0x7f);
    if shift >= 32 || u32::MAX >> shift < add {
        return Err(Error);
    }
    value.checked_add(add << shift).ok_or(Error)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "each byte pushed holds at most eight bits of `value`"
)]
fn encode_integer(out: &mut Vec<u8>, bits: u8, prefix: u32, value: usize) {
    let limit = (1_usize << prefix) - 1;
    if value < limit {
        out.push(bits | value as u8);
        return;
    }
    out.push(bits | limit as u8);
    let mut rest = value - limit;
    while rest >= 128 {
        out.push(0x80 | (rest & 0x7f) as u8);
        rest >>= 7;
    }
    out.push(rest as u8);
}

/// Huffman-coded when that is shorter.
fn encode_string(out: &mut Vec<u8>, text: &[u8]) {
    let length = huffman_length(text);
    if length < text.len() {
        encode_integer(out, 0x80, 7, length);
        huffman_encode(text, out);
    } else {
        encode_integer(out, 0, 7, text.len());
        out.extend_from_slice(text);
    }
}

/// One `decode` call's outcome.
#[derive(Debug, PartialEq, Eq)]
pub struct Step<'a> {
    /// Input bytes consumed.
    pub read: usize,
    /// The header field decoded, valid until the next call.
    pub header: Option<(&'a [u8], &'a [u8])>,
    /// The header block ended: the input was its last and is all consumed.
    pub done: bool,
}

impl Step<'_> {
    /// `nghttp2_hd_inflate_hd`'s flags: `FINAL` (1) and `EMIT` (2).
    #[must_use]
    pub fn flags(&self) -> u32 {
        u32::from(self.done) | (u32::from(self.header.is_some()) << 1)
    }
}

/// What an integer being read (RFC 7541 5.1) counts.
#[derive(Clone, Copy)]
enum Integer {
    TableSize,
    /// An indexed field (6.1).
    Index,
    /// A literal's name index, 0 for a new name (6.2).
    Name,
    /// A string's length (5.2): the value's or the name's.
    Length {
        value: bool,
        huffman: bool,
    },
}

/// Where the decoder stands inside a representation, which fragments may
/// split anywhere, as `nghttp2_hd_inflate_hd`'s states do.
#[derive(Clone, Copy)]
enum State {
    Opcode,
    Integer {
        integer: Integer,
        value: u32,
        shift: u32,
    },
    /// A string's first byte is next: the value's or the name's.
    Length {
        value: bool,
    },
    /// `left` bytes of a string remain.
    Text {
        value: bool,
        huffman: bool,
        left: usize,
    },
}

/// The header block decoder (`nghttp2_hd_inflater`), whose table limit is
/// `SETTINGS_HEADER_TABLE_SIZE`'s default.
pub struct Decoder {
    table: Table,
    state: State,
    /// A representation opened this block, so no table size update may follow.
    in_block: bool,
    /// The literal being read is added to the table.
    indexing: bool,
    huffman: Bits,
    bad: bool,
    name: Vec<u8>,
    value: Vec<u8>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            table: Table::new(DEFAULT_TABLE_SIZE),
            state: State::Opcode,
            in_block: false,
            indexing: false,
            huffman: Bits::default(),
            bad: false,
            name: Vec::new(),
            value: Vec::new(),
        }
    }
}

impl Decoder {
    /// Decode up to the next header field of a block (`nghttp2_hd_inflate_hd`):
    /// call again with the rest of the input until it is consumed, and with
    /// `last` on the block's final fragment. A representation cut short by a
    /// fragment's end is consumed and finished by the next call.
    ///
    /// # Errors
    /// Malformed input, or any call after one that failed.
    pub fn decode(&mut self, input: &[u8], last: bool) -> Result<Step<'_>, Error> {
        if self.bad {
            return Err(Error);
        }
        match self.step(input, last) {
            Ok((read, emitted)) => Ok(Step {
                read,
                header: emitted.then_some((self.name.as_slice(), self.value.as_slice())),
                done: last && !emitted && read == input.len(),
            }),
            Err(error) => {
                self.bad = true;
                Err(error)
            }
        }
    }

    /// The bytes the dynamic table holds (RFC 7541 4.1).
    #[must_use]
    pub fn table_size(&self) -> usize {
        self.table.size
    }

    /// The block ended (`nghttp2_hd_inflate_end_headers`): the next may open
    /// with table size updates.
    pub fn end_headers(&mut self) {
        self.state = State::Opcode;
        self.in_block = false;
    }

    /// The bytes read, and whether they ended in a field.
    fn step(&mut self, input: &[u8], last: bool) -> Result<(usize, bool), Error> {
        let mut rest = input;
        loop {
            if let State::Text {
                value,
                huffman,
                left,
            } = self.state
            {
                let (text, after) = rest.split_at(left.min(rest.len()));
                rest = after;
                let out = if value {
                    &mut self.value
                } else {
                    &mut self.name
                };
                if huffman {
                    self.huffman.decode(text, out)?;
                } else {
                    out.extend_from_slice(text);
                }
                let left = left - text.len();
                self.state = State::Text {
                    value,
                    huffman,
                    left,
                };
                if left > 0 {
                    break;
                }
                if huffman {
                    self.huffman.finish()?;
                }
                if value {
                    return Ok((input.len() - rest.len(), self.emit()));
                }
                self.state = State::Length { value: true };
                continue;
            }
            let Some((&byte, after)) = rest.split_first() else {
                break;
            };
            rest = after;
            let (integer, value, shift, last_byte) = match self.state {
                State::Opcode => {
                    let (integer, prefix) = match byte {
                        0x80.. => (Integer::Index, 7),
                        0x40.. => (Integer::Name, 6),
                        0x20.. if self.in_block => return Err(Error),
                        0x20.. => (Integer::TableSize, 5),
                        _ => (Integer::Name, 4),
                    };
                    self.in_block |= !matches!(integer, Integer::TableSize);
                    self.indexing = byte & 0xc0 == 0x40;
                    let limit = (1 << prefix) - 1;
                    (integer, u32::from(byte & limit), 0, byte & limit < limit)
                }
                State::Length { value } => {
                    let huffman = byte & 0x80 != 0;
                    let integer = Integer::Length { value, huffman };
                    (integer, u32::from(byte & 0x7f), 0, byte & 0x7f < 0x7f)
                }
                State::Integer {
                    integer,
                    value,
                    shift,
                } => (
                    integer,
                    continue_integer(value, shift, byte)?,
                    shift + 7,
                    byte & 0x80 == 0,
                ),
                State::Text { .. } => unreachable!("read above"),
            };
            let max = match integer {
                Integer::TableSize => DEFAULT_TABLE_SIZE,
                Integer::Index | Integer::Name => self.table.len(),
                Integer::Length { .. } => MAX_STRING,
            };
            // nghttp2 bounds a partial integer too.
            let checked = usize::try_from(value)
                .ok()
                .filter(|&checked| checked <= max)
                .ok_or(Error)?;
            if !last_byte {
                self.state = State::Integer {
                    integer,
                    value,
                    shift,
                };
                continue;
            }
            if self.integer(integer, checked)? {
                return Ok((input.len() - rest.len(), true));
            }
        }
        if last && !matches!(self.state, State::Opcode) {
            return Err(Error);
        }
        Ok((input.len(), false))
    }

    /// Act on a complete integer: whether that emitted a field.
    fn integer(&mut self, integer: Integer, value: usize) -> Result<bool, Error> {
        self.state = State::Opcode;
        match integer {
            Integer::TableSize => self.table.resize(value),
            Integer::Index => {
                let (name, field) = self.table.get(value).ok_or(Error)?;
                self.name.clear();
                self.name.extend_from_slice(name);
                self.value.clear();
                self.value.extend_from_slice(field);
                return Ok(true);
            }
            Integer::Name if value == 0 => self.state = State::Length { value: false },
            Integer::Name => {
                let (name, _) = self.table.get(value).ok_or(Error)?;
                self.name.clear();
                self.name.extend_from_slice(name);
                self.state = State::Length { value: true };
            }
            Integer::Length { value: true, .. } if value == 0 => {
                self.value.clear();
                return Ok(self.emit());
            }
            Integer::Length {
                value: is_value,
                huffman,
            } => {
                if is_value {
                    &mut self.value
                } else {
                    &mut self.name
                }
                .clear();
                self.huffman = Bits::default();
                self.state = State::Text {
                    value: is_value,
                    huffman,
                    left: value,
                };
            }
        }
        Ok(false)
    }

    /// A literal is complete: index it if asked.
    fn emit(&mut self) -> bool {
        self.state = State::Opcode;
        if self.indexing {
            self.table.insert(&self.name, &self.value);
        }
        true
    }
}

/// The header block encoder (`nghttp2_hd_deflater`).
pub struct Encoder {
    table: Table,
    /// The most the table may use, whatever the peer allows.
    limit: usize,
    /// The smallest size since the last block, which a shrink-then-grow must
    /// signal too.
    smallest: usize,
    /// A table size update opens the next block.
    notify: bool,
}

impl Encoder {
    /// An encoder whose table never exceeds `limit` (nor the 4096-byte default
    /// until the peer allows more).
    #[must_use]
    pub fn new(limit: u32) -> Self {
        let limit = limit as usize;
        Self {
            table: Table::new(limit.min(DEFAULT_TABLE_SIZE)),
            limit,
            smallest: usize::MAX,
            notify: limit < DEFAULT_TABLE_SIZE,
        }
    }

    /// The bytes the dynamic table holds (RFC 7541 4.1).
    #[must_use]
    pub fn table_size(&self) -> usize {
        self.table.size
    }

    /// The peer's `SETTINGS_HEADER_TABLE_SIZE` changed.
    pub fn set_table_size(&mut self, size: u32) {
        let size = (size as usize).min(self.limit);
        self.table.resize(size);
        self.smallest = self.smallest.min(size);
        self.notify = true;
    }

    /// Append the block coding `headers` to `out`.
    pub fn encode<'a>(
        &mut self,
        headers: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
        out: &mut Vec<u8>,
    ) {
        if std::mem::take(&mut self.notify) {
            let smallest = std::mem::replace(&mut self.smallest, usize::MAX);
            if self.table.max > smallest {
                encode_integer(out, 0x20, 5, smallest);
            }
            encode_integer(out, 0x20, 5, self.table.max);
        }
        for (name, value) in headers {
            self.field(name, value, out);
        }
    }

    /// nghttp2's choices: never index credentials or short cookies, don't
    /// index fields unlikely to repeat or too big for the table, and prefer an
    /// exact dynamic match, then the static table, then a dynamic name.
    fn field(&mut self, name: &[u8], value: &[u8], out: &mut Vec<u8>) {
        let never = name == b"authorization" || name == b"cookie" && value.len() < 20;
        let indexing = !never
            && !matches!(
                name,
                b":path"
                    | b"age"
                    | b"content-length"
                    | b"etag"
                    | b"if-modified-since"
                    | b"if-none-match"
                    | b"location"
                    | b"set-cookie"
            )
            && ENTRY_OVERHEAD + name.len() + value.len() <= self.table.max * 3 / 4;
        let exact = (!never)
            .then(|| self.table.find(name, Some(value)))
            .flatten()
            .or_else(|| (!never).then(|| static_index(name, Some(value))).flatten());
        if let Some(index) = exact {
            encode_integer(out, 0x80, 7, index);
            return;
        }
        let index = static_index(name, None).or_else(|| self.table.find(name, None));
        if indexing {
            self.table.insert(name, value);
        }
        let (bits, prefix) = match (indexing, never) {
            (true, _) => (0x40, 6),
            (false, true) => (0x10, 4),
            (false, false) => (0, 4),
        };
        if let Some(index) = index {
            encode_integer(out, bits, prefix, index);
        } else {
            out.push(bits);
            encode_string(out, name);
        }
        encode_string(out, value);
    }
}
