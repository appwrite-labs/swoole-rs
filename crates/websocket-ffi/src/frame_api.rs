// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/protocol/websocket.cc (Swoole contributors).
//! `src/protocol/websocket.cc`'s codec entry points over `swoole::String` and
//! `websocket::Frame`, as safe functions over slices. Protocol tables hold
//! [`get_package_length`], which `upstream-symbols` also defines under its upstream
//! (Itanium) symbol.
use std::ffi::{CStr, c_char, c_int};
use std::mem::MaybeUninit;
use swoole_rs_corebase::CCast;

use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::log;
use swoole_rs_corebase::net_types::{PacketLength, Protocol};

use swoole_rs_websocket::{
    DecodedFrame, INVALID_CONTROL, control_invalid, decode_in_place, encode_close_into,
    encode_parts, encoded_size, frame_length,
};

/// `SW_LOG_WARNING`.
const LOG_WARNING: c_int = 4;
/// `SW_ERROR_WEBSOCKET_INCOMPLETE_PACKET`.
const INCOMPLETE_PACKET: c_int = 8507;
/// `FLAG_FIN`, `FLAG_MASK`, `FLAG_ENCODE_HEADER_ONLY`.
const FIN: u8 = 1;
const MASK: u8 = 32;
const HEADER_ONLY: u8 = 64;
/// `OPCODE_CLOSE`.
const OPCODE_CLOSE: u8 = 8;
/// `SW_WEBSOCKET_CLOSE_CODE_LEN`, `SW_WEBSOCKET_CLOSE_REASON_MAX_LEN`.
const CLOSE_CODE_LEN: usize = 2;
const CLOSE_REASON_MAX_LEN: usize = 125;

/// `websocket::Frame`. `header` holds `Header`'s bitfields: OPCODE:4, RSV3, RSV2,
/// RSV1, FIN in the first byte (low bit first), LENGTH:7 and MASK in the second.
#[repr(C)]
pub struct Frame {
    pub header: [u8; 2],
    pub mask_key: [u8; 4],
    pub header_length: u16,
    pub payload_length: usize,
    pub payload: *mut c_char,
}

const _: () = assert!(
    size_of::<Frame>() == 24 && std::mem::offset_of!(Frame, payload_length) == 8,
    "websocket::Frame's layout"
);

/// Upstream encoders fail a masked header-only frame after appending it.
fn header_only_mask(flags: u8) -> bool {
    flags & MASK != 0 && flags & HEADER_ONLY != 0
}

/// `encode`/`pack_close_frame`'s tail: reserve `size` bytes past the buffer's
/// length, let `write` fill them, and count them only when it wrote `size`.
fn append_frame(
    buffer: &mut CppString,
    size: usize,
    write: impl FnOnce(&mut [MaybeUninit<u8>]) -> usize,
) -> bool {
    let length = buffer.len();
    if size == 0 || size > usize::MAX - length {
        return false;
    }
    if length + size > buffer.size() {
        buffer.reserve(length + size);
    }
    let Some(spare) = buffer.spare().get_mut(..size) else {
        return false;
    };
    if write(spare) != size {
        return false;
    }
    // SAFETY: `write` filled the `size` bytes after the content, inside the buffer.
    unsafe { buffer.set_len(length + size) };
    true
}

fn control_warning(origin: &CStr, opcode: u8, fin: u8, length: usize) {
    log::warning(
        origin,
        format_args!(
            "invalid websocket control frame: opcode={opcode}, fin={fin}, payload_length={length}"
        ),
    );
}

/// `decode`'s checks and warnings over `length` writable bytes; the payload is
/// unmasked in place. `Err` carries the decoded header of a rejected frame and
/// whether its lengths were filled in (a compressed control frame).
///
/// # Errors
/// An incomplete frame or a compressed control frame, after upstream's warning.
///
#[inline]
pub fn decode_frame(data: &mut [u8]) -> Result<DecodedFrame, (DecodedFrame, bool)> {
    let length = data.len();
    let (total, decoded) = decode_in_place(data);
    if total == INVALID_CONTROL {
        log::warning(
            c"websocket::decode",
            format_args!(
                "invalid compressed websocket control frame received: opcode={}",
                decoded.opcode
            ),
        );
        return Err((decoded, true));
    }
    if total <= 0 || usize::try_from(total).is_ok_and(|total| length < total) {
        log::error_log(
            LOG_WARNING,
            INCOMPLETE_PACKET,
            c"websocket::decode",
            format_args!(
                "incomplete packet, expected length is {}, actual length is {length}",
                total.cast_unsigned()
            ),
        );
        return Err((decoded, false));
    }
    Ok(decoded)
}

/// `ssize_t get_package_length(const Protocol *, Socket *, PacketLength *)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZN6swoole9websocket18get_package_lengthEPKNS_8ProtocolEPNS_7network6SocketEPNS_12PacketLengthE"
    )
)]
pub extern "C" fn get_package_length(
    _protocol: *const Protocol,
    _socket: *mut std::ffi::c_void,
    pl: &mut PacketLength,
) -> isize {
    let bytes = pl.bytes();
    if bytes.len() < 2 {
        return 0;
    }
    let (length, header) = frame_length(bytes);
    if let Some(header) = header {
        pl.header_len = header;
    }
    length.c_cast::<isize>()
}

/// `bool encode(String *buffer, const char *data, size_t length, uint8_t opcode, uint8_t flags)`
/// over the payload's bytes. A header-only frame takes their count and writes none.
pub fn encode(buffer: &mut CppString, data: &[u8], opcode: u8, flags: u8) -> bool {
    let length = data.len();
    if control_invalid(opcode, flags, length) {
        control_warning(
            c"websocket::encode",
            opcode,
            u8::from(flags & FIN != 0),
            length,
        );
        return false;
    }
    let size = encoded_size(length, flags);
    let data = if flags & HEADER_ONLY != 0 { &[] } else { data };
    append_frame(buffer, size, |out| {
        encode_parts(length, data, opcode, flags, out)
    }) && !header_only_mask(flags)
}

/// `bool decode(Frame *frame, char *data, size_t length)`: `frame.payload` points
/// into `data`.
pub fn decode(frame: &mut Frame, data: &mut [u8]) -> bool {
    let result = decode_frame(data);
    let (decoded, ok, filled) = match result {
        Ok(decoded) => (decoded, true, true),
        Err((decoded, filled)) => (decoded, false, filled),
    };
    let flags = decoded.flags;
    // Header's bitfields: FIN 1 -> bit 7, RSV1 4 -> 6, RSV2 8 -> 5, RSV3 16 -> 4.
    frame.header[0] = (decoded.opcode & 15)
        | (flags & 16)
        | ((flags & 8) << 2)
        | ((flags & 4) << 4)
        | ((flags & FIN) << 7);
    frame.header[1] = (decoded.length_code & 127) | ((flags & MASK) << 2);
    if filled {
        let header_length = decoded.header_length.c_cast::<u16>();
        frame.header_length = header_length;
        frame.payload_length = decoded.payload_length;
    }
    if ok {
        frame.payload = if decoded.payload_length == 0 {
            std::ptr::null_mut()
        } else {
            data.as_mut_ptr().wrapping_add(decoded.header_length).cast()
        };
        if decoded.payload_length != 0 && flags & MASK != 0 {
            frame.mask_key = decoded.mask_key;
        }
    }
    ok
}

/// `bool pack_close_frame(String *buffer, int code, const char *reason, size_t length, uint8_t flags)`.
pub fn pack_close_frame(buffer: &mut CppString, code: c_int, reason: &[u8], flags: u8) -> bool {
    let origin = c"websocket::pack_close_frame";
    let length = reason.len();
    if length > CLOSE_REASON_MAX_LEN {
        log::warning(
            origin,
            format_args!("the max length of close reason is {CLOSE_REASON_MAX_LEN}"),
        );
        return false;
    }
    if control_invalid(OPCODE_CLOSE, FIN, CLOSE_CODE_LEN + length) {
        control_warning(origin, OPCODE_CLOSE, 1, CLOSE_CODE_LEN + length);
        return false;
    }
    let size = encoded_size(length + CLOSE_CODE_LEN, flags | FIN);
    append_frame(buffer, size, |out| {
        encode_close_into(code, reason, flags, out)
    }) && !header_only_mask(flags)
}

/// `void print_frame(const Frame *frame)`, of a frame's `header` and its payload's bytes.
pub fn print_frame(header: [u8; 2], payload: &[u8]) {
    let [first, second] = header;
    // `sw_printf`: Swoole's stdout stream, which swoole_set_stdout_stream replaces.
    let mut line = format!(
        "FIN: {:x}, RSV1: {}, RSV2: {}, RSV3: {}, opcode: {}, MASK: {}, length: {}\n",
        first >> 7,
        (first >> 6) & 1,
        (first >> 5) & 1,
        (first >> 4) & 1,
        first & 15,
        second >> 7,
        payload.len().cast_signed(),
    )
    .into_bytes();
    if !payload.is_empty() {
        // `%.*s` stops at the first NUL.
        let text = payload.split(|&byte| byte == 0).next().unwrap_or(payload);
        line.extend_from_slice(b"payload: ");
        line.extend_from_slice(text);
        line.push(b'\n');
    }
    swoole_rs_corebase::sw_print(&line);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string() -> CppString {
        CppString::EMPTY
    }

    /// A masked text frame decodes to its payload, header bits and mask key.
    #[test]
    fn encode_then_decode_restores_the_frame() {
        let mut buffer = string();
        let text = b"hello world";
        assert!(encode(&mut buffer, text, 1, FIN | MASK));
        let mut frame: Frame = unsafe { std::mem::zeroed() };
        let mut wire = buffer.view().to_vec();
        assert!(decode(&mut frame, &mut wire));
        assert_eq!(frame.header[0], 0x81, "FIN and OPCODE_TEXT");
        assert_eq!(frame.header[1] >> 7, 1, "MASK");
        assert_eq!(frame.header_length, 6);
        assert_eq!(frame.payload_length, text.len());
        let payload =
            unsafe { std::slice::from_raw_parts(frame.payload.cast::<u8>(), frame.payload_length) };
        assert_eq!(payload, text);
        assert_eq!(&frame.mask_key, b"258E");
        buffer.dispose();
    }

    /// A header-only masked frame is appended and then reported as a failure.
    #[test]
    fn masked_header_only_appends_but_fails() {
        let mut buffer = string();
        assert!(!encode(&mut buffer, &[0; 10], 2, FIN | MASK | HEADER_ONLY));
        assert_eq!(buffer.len(), 6);
        buffer.dispose();
    }

    /// Control frames over 125 bytes and close reasons over the limit are refused
    /// without touching the buffer.
    #[test]
    fn invalid_control_frames_are_refused() {
        let mut buffer = string();
        let payload = [0u8; 126];
        assert!(!encode(&mut buffer, &payload, 9, FIN));
        assert!(!pack_close_frame(&mut buffer, 1000, &payload, FIN));
        assert!(!pack_close_frame(&mut buffer, 1000, &payload[..124], FIN));
        assert_eq!(buffer.len(), 0);
        assert!(pack_close_frame(&mut buffer, 1000, b"bye", FIN));
        assert_eq!(buffer.view(), b"\x88\x05\x03\xe8bye");
        buffer.dispose();
    }

    /// An incomplete frame fails and leaves the lengths alone; a compressed
    /// control frame fails with its lengths filled in.
    #[test]
    fn rejected_frames_fill_what_upstream_fills() {
        let mut frame: Frame = unsafe { std::mem::zeroed() };
        let mut short = *b"\x81\x05he";
        assert!(!decode(&mut frame, &mut short));
        assert_eq!(
            (frame.header[0], frame.header[1], frame.payload_length),
            (0x81, 5, 0)
        );
        let mut ping = *b"\xc9\x01x";
        assert!(!decode(&mut frame, &mut ping));
        assert_eq!(
            (frame.header[0], frame.header_length, frame.payload_length),
            (0xc9, 2, 1)
        );
    }

    // core-tests/src/server/http_server.cpp: websocket_encode. `print_frame` writes to
    // Swoole's stdout stream, pointed at a log file for the case.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no fopen")]
    fn websocket_encode() {
        let mut buffer = string();
        let packet = b"hello world\n";
        assert!(encode(&mut buffer, packet, 1, FIN | MASK));
        let mut ws: Frame = unsafe { std::mem::zeroed() };
        let mut wire = buffer.view().to_vec();
        assert!(decode(&mut ws, &mut wire));

        let log = std::env::temp_dir().join(format!(
            "swoole-websocket-encode-{}.log",
            std::process::id()
        ));
        let path = std::ffi::CString::new(log.to_str().unwrap()).unwrap();
        let fp = unsafe { libc::fopen(path.as_ptr(), c"a+".as_ptr()) };
        assert!(!fp.is_null());
        let ori_fp = swoole_rs_corebase::swoole_get_stdout_stream();
        swoole_rs_corebase::swoole_set_stdout_stream(fp.cast());
        let payload = &wire[usize::from(ws.header_length)..][..ws.payload_length];
        print_frame(ws.header, payload);
        unsafe { libc::fclose(fp) };
        swoole_rs_corebase::swoole_set_stdout_stream(ori_fp);

        let rs = std::fs::read_to_string(&log).unwrap();
        assert!(rs.contains("FIN: 1,"), "{rs}");
        assert!(rs.contains("RSV1: 0,"), "{rs}");
        assert!(rs.contains("opcode: 1,"), "{rs}");
        assert!(rs.contains("payload: hello world\n"), "{rs}");
        std::fs::remove_file(&log).unwrap();
        buffer.dispose();
    }

    // core-tests/src/server/http_server.cpp: websocket_compressed_control_frame.
    #[test]
    fn websocket_compressed_control_frame() {
        /// `FLAG_RSV1`; `OPCODE_TEXT`, `OPCODE_PING`, `OPCODE_PONG`.
        const RSV1: u8 = 4;
        let mut buffer = string();
        let mut frame: Frame = unsafe { std::mem::zeroed() };
        for opcode in [OPCODE_CLOSE, 9, 10] {
            buffer.clear();
            assert!(encode(&mut buffer, b"", opcode, FIN | RSV1));
            assert!(!decode(&mut frame, &mut buffer.view().to_vec()), "{opcode}");
        }
        buffer.clear();
        assert!(encode(&mut buffer, b"", 1, FIN | RSV1));
        assert!(decode(&mut frame, &mut buffer.view().to_vec()));
        buffer.dispose();
    }

    #[test]
    fn package_length_reports_the_header() {
        let data = *b"\x82\x7e\x01\x00";
        let mut pl = PacketLength::new(&data);
        assert_eq!(
            get_package_length(std::ptr::null(), std::ptr::null_mut(), &mut pl),
            260
        );
        assert_eq!(pl.header_len, 4);
    }
}
