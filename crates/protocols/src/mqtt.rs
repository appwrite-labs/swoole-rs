/// Parse MQTT remaining-length bytes; packet type and nonminimal representations
/// are intentionally not validated, matching the pinned protocol.
#[must_use]
pub fn mqtt_frame_length(bytes: &[u8]) -> i64 {
    let mut length = 0;
    for (position, byte) in (0i64..).zip(bytes.iter().skip(1).take(4)) {
        length += i64::from(byte & 127) << (7 * position);
        if byte & 128 == 0 {
            return length + position + 2;
        }
        if position == 3 {
            return -1;
        }
    }
    0
}

/// # Safety
/// Input covers `length` readable bytes; null is allowed at zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_mqtt_frame_length(data: *const u8, length: usize) -> i64 {
    // SAFETY: `data` lends `length` initialized bytes (null only when empty) for this call.
    unsafe { crate::input(data, length) }.map_or(-1, mqtt_frame_length)
}

/// `SW_MQTT_MAX_LENGTH_SIZE`.
const MAX_LENGTH_SIZE: u8 = 4;

/// `swoole::PacketLength`.
#[repr(C)]
pub struct PacketLength {
    pub buf: *const u8,
    pub buf_size: u32,
    pub header_len: u32,
}

/// `swoole::mqtt::set_protocol(protocol)`.
///
/// # Safety
/// `protocol` is a live, writable `swoole::Protocol`.
#[unsafe(export_name = "_ZN6swoole4mqtt12set_protocolEPNS_8ProtocolE")]
pub unsafe extern "C" fn set_protocol(protocol: *mut swoole_rs_corebase::net_types::Protocol) {
    // SAFETY: as the caller promises.
    let protocol = unsafe { &mut *protocol };
    protocol.package_length_size = MAX_LENGTH_SIZE;
    protocol.package_length_offset = 1;
    protocol.package_body_offset = 0;
    protocol.get_package_length = get_package_length as *mut core::ffi::c_void;
}

/// `swoole::mqtt::get_package_length(protocol, socket, pl)`: 0 until the remaining
/// length is buffered, then the whole packet's length; -1, with a warning, past four
/// length bytes.
///
/// # Safety
/// `pl` is valid and its `buf` addresses `buf_size` initialized bytes.
#[unsafe(export_name = "_ZN6swoole4mqtt18get_package_lengthEPKNS_8ProtocolEPNS_7network6SocketEPNS_12PacketLengthE")]
pub unsafe extern "C" fn get_package_length(
    _protocol: *const core::ffi::c_void,
    _socket: *mut core::ffi::c_void,
    pl: *const PacketLength,
) -> isize {
    // SAFETY: as the caller promises.
    let pl = unsafe { &*pl };
    // SAFETY: `buf` addresses `buf_size` initialized bytes.
    let length = unsafe { swr_v1_mqtt_frame_length(pl.buf, pl.buf_size as usize) };
    if length < 0 {
        swoole_rs_corebase::log::error_log(
            /* SW_LOG_WARNING */ 4,
            /* SW_ERROR_PACKAGE_LENGTH_TOO_LARGE */ 1201,
            c"mqtt::get_package_length",
            format_args!("bad request, the variable header size is larger than {MAX_LENGTH_SIZE}"),
        );
    }
    isize::try_from(length).unwrap_or(-1)
}

#[cfg(test)]
#[expect(
    clippy::cast_possible_wrap,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_prefixes_and_nonminimal_lengths() {
        for payload in [
            0u32,
            1,
            127,
            128,
            16383,
            16384,
            2_097_151,
            2_097_152,
            268_435_455,
        ] {
            let mut packet = vec![0x30];
            let mut value = payload;
            loop {
                let mut byte = (value % 128) as u8;
                value /= 128;
                if value != 0 {
                    byte |= 128;
                }
                packet.push(byte);
                if value == 0 {
                    break;
                }
            }
            for size in 0..packet.len() {
                assert_eq!(mqtt_frame_length(&packet[..size]), 0);
            }
            assert_eq!(
                mqtt_frame_length(&packet),
                i64::from(payload) + packet.len() as i64
            );
        }
        assert_eq!(mqtt_frame_length(&[0, 0x80, 0]), 3);
        assert_eq!(mqtt_frame_length(&[0, 0x80, 0x80, 0x80, 0x80]), -1);
    }

    #[test]
    fn upstream_symbols_set_the_protocol_and_measure_packets() {
        let mut protocol: swoole_rs_corebase::net_types::Protocol = unsafe { core::mem::zeroed() };
        unsafe { set_protocol(&raw mut protocol) };
        assert_eq!(
            (
                protocol.package_length_size,
                protocol.package_length_offset,
                protocol.package_body_offset
            ),
            (4, 1, 0)
        );
        assert_eq!(protocol.get_package_length, get_package_length as *mut _);
        let measure = |bytes: &[u8]| {
            let pl = PacketLength {
                buf: bytes.as_ptr(),
                buf_size: u32::try_from(bytes.len()).unwrap(),
                header_len: 99,
            };
            let length = unsafe {
                get_package_length(core::ptr::null(), core::ptr::null_mut(), &raw const pl)
            };
            assert_eq!(pl.header_len, 99);
            length
        };
        assert_eq!(measure(&[0x30]), 0);
        assert_eq!(measure(&[0x30, 0x80, 0]), 3);
        assert_eq!(measure(&[0x30, 0x80, 0x80, 0x80, 0x80]), -1);
    }
}
