// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/protocol/websocket.cc (Swoole contributors).
//! `src/protocol/websocket.cc`'s codec entry points over `swoole::String` and
//! `websocket::Frame`. With `upstream-symbols` each is defined under its upstream
//! (Itanium) symbol, and the file leaves the build.
use std::ffi::{CStr, c_char, c_int};

use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::log;

use swoole_rs_websocket::{
    DecodedFrame, INVALID_CONTROL, control_invalid, decode_in_place, encoded_size, frame_length,
};

use crate::{swr_v1_ws_encode, swr_v1_ws_encode_close, swr_v1_ws_mask};

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

/// `swoole::PacketLength`.
#[repr(C)]
pub struct PacketLength {
    pub buf: *const u8,
    pub buf_size: u32,
    pub header_len: u32,
}

/// Upstream encoders fail a masked header-only frame after appending it.
fn header_only_mask(flags: u8) -> bool {
    flags & MASK != 0 && flags & HEADER_ONLY != 0
}

/// `encode`/`pack_close_frame`'s tail: reserve `size` bytes past the buffer's
/// length, let `write` fill them, and count them only when it wrote `size`.
fn append_frame(buffer: &mut CppString, size: usize, write: impl FnOnce(*mut u8) -> usize) -> bool {
    if size == 0 || size > usize::MAX - buffer.length {
        return false;
    }
    if buffer.length + size > buffer.size {
        buffer.reserve(buffer.length + size);
    }
    if write(buffer.str.wrapping_add(buffer.length)) != size {
        return false;
    }
    buffer.length += size;
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
/// # Safety
/// `data` covers `length` writable bytes (null only at length zero).
#[inline]
pub unsafe fn decode_frame(
    data: *mut u8,
    length: usize,
) -> Result<DecodedFrame, (DecodedFrame, bool)> {
    let data = if length == 0 {
        &mut []
    } else {
        // SAFETY: as the caller promises.
        unsafe { std::slice::from_raw_parts_mut(data, length) }
    };
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

macro_rules! upstream {
    ($($symbol:literal => $item:item)*) => {$(
        #[cfg_attr(feature = "upstream-symbols", unsafe(export_name = $symbol))]
        $item
    )*};
}

upstream! {
    "_ZN6swoole9websocket18get_package_lengthEPKNS_8ProtocolEPNS_7network6SocketEPNS_12PacketLengthE" =>
    /// `ssize_t get_package_length(const Protocol *, Socket *, PacketLength *)`.
    ///
    /// # Safety
    /// `pl` is a live packet length over `buf_size` readable bytes.
    pub unsafe extern "C" fn get_package_length(
        _protocol: *const std::ffi::c_void,
        _socket: *mut std::ffi::c_void,
        pl: *mut PacketLength,
    ) -> isize {
        // SAFETY: as the caller promises.
        let pl = unsafe { &mut *pl };
        if pl.buf_size < 2 {
            return 0;
        }
        // SAFETY: the packet's `buf_size` readable bytes.
        let (length, header) = frame_length(unsafe { std::slice::from_raw_parts(pl.buf, pl.buf_size as usize) });
        if let Some(header) = header {
            pl.header_len = header;
        }
        #[expect(clippy::cast_possible_truncation, reason = "a frame length fits ssize_t")]
        let length = length as isize;
        length
    }

    "_ZN6swoole9websocket4maskEPcmPKc" =>
    /// `void mask(char *data, size_t len, const char *mask_key)`.
    ///
    /// # Safety
    /// `data` covers `len` writable bytes and `mask_key` four readable ones.
    pub unsafe extern "C" fn mask(data: *mut c_char, len: usize, mask_key: *const c_char) {
        // SAFETY: as the caller promises.
        if unsafe { swr_v1_ws_mask(data.cast(), len, mask_key.cast()) } != 0 {
            log::warning(c"websocket::mask", format_args!("Rust WebSocket mask rejected input"));
        }
    }

    "_ZN6swoole9websocket6encodeEPNS_6StringEPKcmhh" =>
    /// `bool encode(String *buffer, const char *data, size_t length, uint8_t opcode, uint8_t flags)`.
    ///
    /// # Safety
    /// `buffer` is a live String; `data` covers `length` readable bytes.
    pub unsafe extern "C" fn encode(buffer: *mut CppString, data: *const c_char, length: usize, opcode: u8, flags: u8) -> bool {
        if control_invalid(opcode, flags, length) {
            control_warning(c"websocket::encode", opcode, u8::from(flags & FIN != 0), length);
            return false;
        }
        let size = encoded_size(length, flags);
        // SAFETY: as the caller promises.
        let buffer = unsafe { &mut *buffer };
        // SAFETY: `data`'s bytes and the `size` bytes just reserved.
        append_frame(buffer, size, |out| unsafe { swr_v1_ws_encode(data.cast(), length, opcode, flags, out, size) })
            && !header_only_mask(flags)
    }

    "_ZN6swoole9websocket6decodeEPNS0_5FrameEPcm" =>
    /// `bool decode(Frame *frame, char *data, size_t length)`.
    ///
    /// # Safety
    /// `frame` is writable; `data` covers `length` writable bytes.
    pub unsafe extern "C" fn decode(frame: *mut Frame, data: *mut c_char, length: usize) -> bool {
        // SAFETY: as the caller promises.
        let frame = unsafe { &mut *frame };
        // SAFETY: as the caller promises.
        let result = unsafe { decode_frame(data.cast(), length) };
        let (decoded, ok, filled) = match result {
            Ok(decoded) => (decoded, true, true),
            Err((decoded, filled)) => (decoded, false, filled),
        };
        let flags = decoded.flags;
        // Header's bitfields: FIN 1 -> bit 7, RSV1 4 -> 6, RSV2 8 -> 5, RSV3 16 -> 4.
        frame.header[0] = (decoded.opcode & 15) | (flags & 16) | ((flags & 8) << 2) | ((flags & 4) << 4) | ((flags & FIN) << 7);
        frame.header[1] = (decoded.length_code & 127) | ((flags & MASK) << 2);
        if filled {
            #[expect(clippy::cast_possible_truncation, reason = "Frame::header_length is a uint16_t")]
            let header_length = decoded.header_length as u16;
            frame.header_length = header_length;
            frame.payload_length = decoded.payload_length;
        }
        if ok {
            frame.payload = if decoded.payload_length == 0 {
                std::ptr::null_mut()
            } else {
                data.wrapping_add(decoded.header_length)
            };
            if decoded.payload_length != 0 && flags & MASK != 0 {
                frame.mask_key = decoded.mask_key;
            }
        }
        ok
    }

    "_ZN6swoole9websocket16pack_close_frameEPNS_6StringEiPKcmh" =>
    /// `bool pack_close_frame(String *buffer, int code, const char *reason, size_t length, uint8_t flags)`.
    ///
    /// # Safety
    /// `buffer` is a live String; `reason` covers `length` readable bytes.
    pub unsafe extern "C" fn pack_close_frame(buffer: *mut CppString, code: c_int, reason: *const c_char, length: usize, flags: u8) -> bool {
        let origin = c"websocket::pack_close_frame";
        if length > CLOSE_REASON_MAX_LEN {
            log::warning(origin, format_args!("the max length of close reason is {CLOSE_REASON_MAX_LEN}"));
            return false;
        }
        if control_invalid(OPCODE_CLOSE, FIN, CLOSE_CODE_LEN + length) {
            control_warning(origin, OPCODE_CLOSE, 1, CLOSE_CODE_LEN + length);
            return false;
        }
        let size = encoded_size(length + CLOSE_CODE_LEN, flags | FIN);
        // SAFETY: as the caller promises.
        let buffer = unsafe { &mut *buffer };
        // SAFETY: the reason's bytes and the `size` bytes just reserved.
        append_frame(buffer, size, |out| unsafe { swr_v1_ws_encode_close(code, reason.cast(), length, flags, out, size) })
            && !header_only_mask(flags)
    }

    "_ZN6swoole9websocket11print_frameEPKNS0_5FrameE" =>
    /// `void print_frame(const Frame *frame)`.
    ///
    /// # Safety
    /// `frame` is a live Frame whose payload covers `payload_length` bytes.
    pub unsafe extern "C" fn print_frame(frame: *const Frame) {
        // SAFETY: as the caller promises.
        let frame = unsafe { &*frame };
        let [first, second] = frame.header;
        #[expect(clippy::cast_possible_wrap, reason = "upstream prints a size_t with %ld")]
        let length = frame.payload_length as libc::c_long;
        // `sw_printf`: Swoole's stdout stream, which swoole_set_stdout_stream replaces.
        let out = swoole_rs_corebase::swoole_get_stdout_stream().cast::<libc::FILE>();
        // SAFETY: the stream is live and the arguments match the format.
        unsafe {
            libc::fprintf(
                out,
                c"FIN: %x, RSV1: %d, RSV2: %d, RSV3: %d, opcode: %d, MASK: %d, length: %ld\n".as_ptr(),
                c_int::from(first >> 7),
                c_int::from((first >> 6) & 1),
                c_int::from((first >> 5) & 1),
                c_int::from((first >> 4) & 1),
                c_int::from(first & 15),
                c_int::from(second >> 7),
                length,
            )
        };
        if frame.payload_length != 0 {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap,
                reason = "upstream's (int) precision"
            )]
            let precision = frame.payload_length as c_int;
            // SAFETY: the payload holds `payload_length` bytes.
            unsafe { libc::fprintf(out, c"payload: %.*s\n".as_ptr(), precision, frame.payload) };
        }
    }
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
        assert!(unsafe {
            encode(
                &raw mut buffer,
                text.as_ptr().cast(),
                text.len(),
                1,
                FIN | MASK,
            )
        });
        let mut frame: Frame = unsafe { std::mem::zeroed() };
        assert!(unsafe { decode(&raw mut frame, buffer.str.cast(), buffer.length) });
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
        assert!(!unsafe {
            encode(
                &raw mut buffer,
                std::ptr::null(),
                10,
                2,
                FIN | MASK | HEADER_ONLY,
            )
        });
        assert_eq!(buffer.length, 6);
        buffer.dispose();
    }

    /// Control frames over 125 bytes and close reasons over the limit are refused
    /// without touching the buffer.
    #[test]
    fn invalid_control_frames_are_refused() {
        let mut buffer = string();
        let payload = [0u8; 126];
        assert!(!unsafe { encode(&raw mut buffer, payload.as_ptr().cast(), 126, 9, FIN) });
        assert!(!unsafe {
            pack_close_frame(&raw mut buffer, 1000, payload.as_ptr().cast(), 126, FIN)
        });
        assert!(!unsafe {
            pack_close_frame(&raw mut buffer, 1000, payload.as_ptr().cast(), 124, FIN)
        });
        assert_eq!(buffer.length, 0);
        assert!(unsafe { pack_close_frame(&raw mut buffer, 1000, c"bye".as_ptr(), 3, FIN) });
        assert_eq!(
            unsafe { std::slice::from_raw_parts(buffer.str, buffer.length) },
            b"\x88\x05\x03\xe8bye"
        );
        buffer.dispose();
    }

    /// An incomplete frame fails and leaves the lengths alone; a compressed
    /// control frame fails with its lengths filled in.
    #[test]
    fn rejected_frames_fill_what_upstream_fills() {
        let mut frame: Frame = unsafe { std::mem::zeroed() };
        let mut short = *b"\x81\x05he";
        assert!(!unsafe { decode(&raw mut frame, short.as_mut_ptr().cast(), short.len()) });
        assert_eq!(
            (frame.header[0], frame.header[1], frame.payload_length),
            (0x81, 5, 0)
        );
        let mut ping = *b"\xc9\x01x";
        assert!(!unsafe { decode(&raw mut frame, ping.as_mut_ptr().cast(), ping.len()) });
        assert_eq!(
            (frame.header[0], frame.header_length, frame.payload_length),
            (0xc9, 2, 1)
        );
    }

    // core-tests/src/server/http_server.cpp: websocket_encode. `print_frame` writes to
    // Swoole's stdout stream, pointed at a log file for the case.
    #[test]
    fn websocket_encode() {
        let mut buffer = string();
        let packet = b"hello world\n";
        assert!(unsafe {
            encode(
                &raw mut buffer,
                packet.as_ptr().cast(),
                packet.len(),
                1,
                FIN | MASK,
            )
        });
        let mut ws: Frame = unsafe { std::mem::zeroed() };
        assert!(unsafe { decode(&raw mut ws, buffer.str.cast(), buffer.length) });

        let log = std::env::temp_dir().join(format!(
            "swoole-websocket-encode-{}.log",
            std::process::id()
        ));
        let path = std::ffi::CString::new(log.to_str().unwrap()).unwrap();
        let fp = unsafe { libc::fopen(path.as_ptr(), c"a+".as_ptr()) };
        assert!(!fp.is_null());
        let ori_fp = swoole_rs_corebase::swoole_get_stdout_stream();
        swoole_rs_corebase::swoole_set_stdout_stream(fp.cast());
        unsafe { print_frame(&raw const ws) };
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
            buffer.length = 0;
            assert!(unsafe { encode(&raw mut buffer, c"".as_ptr(), 0, opcode, FIN | RSV1) });
            assert!(
                !unsafe { decode(&raw mut frame, buffer.str.cast(), buffer.length) },
                "{opcode}"
            );
        }
        buffer.length = 0;
        assert!(unsafe { encode(&raw mut buffer, c"".as_ptr(), 0, 1, FIN | RSV1) });
        assert!(unsafe { decode(&raw mut frame, buffer.str.cast(), buffer.length) });
        buffer.dispose();
    }

    #[test]
    fn package_length_reports_the_header() {
        let data = *b"\x82\x7e\x01\x00";
        let mut pl = PacketLength {
            buf: data.as_ptr(),
            buf_size: 4,
            header_len: 0,
        };
        assert_eq!(
            unsafe { get_package_length(std::ptr::null(), std::ptr::null_mut(), &raw mut pl) },
            260
        );
        assert_eq!(pl.header_len, 4);
    }
}
