//! MQTT's C ABI and its `swoole::mqtt` entries under their upstream symbols, over
//! [`mqtt_frame_length`].
use swoole_rs_corebase::net_types::{PacketLength, Protocol};
use swoole_rs_protocols::mqtt_frame_length;

/// # Safety
/// Input covers `length` readable bytes; null is allowed at zero length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_mqtt_frame_length(data: *const u8, length: usize) -> i64 {
    // SAFETY: `data` lends `length` initialized bytes (null only when empty) for this call.
    unsafe { crate::input(data, length) }.map_or(-1, mqtt_frame_length)
}

/// `SW_MQTT_MAX_LENGTH_SIZE`.
const MAX_LENGTH_SIZE: u8 = 4;

/// `swoole::mqtt::set_protocol(protocol)`.
#[unsafe(export_name = "_ZN6swoole4mqtt12set_protocolEPNS_8ProtocolE")]
pub extern "C" fn set_protocol(protocol: &mut Protocol) {
    protocol.package_length_size = MAX_LENGTH_SIZE;
    protocol.package_length_offset = 1;
    protocol.package_body_offset = 0;
    protocol.get_package_length = Some(get_package_length);
}

/// `swoole::mqtt::get_package_length(protocol, socket, pl)`: 0 until the remaining
/// length is buffered, then the whole packet's length; -1, with a warning, past four
/// length bytes.
#[unsafe(export_name = "_ZN6swoole4mqtt18get_package_lengthEPKNS_8ProtocolEPNS_7network6SocketEPNS_12PacketLengthE")]
pub extern "C" fn get_package_length(
    _protocol: *const Protocol,
    _socket: *mut core::ffi::c_void,
    pl: &mut PacketLength,
) -> isize {
    let length = mqtt_frame_length(pl.bytes());
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
mod tests {
    use super::*;

    #[test]
    fn upstream_symbols_set_the_protocol_and_measure_packets() {
        let mut protocol = Protocol::EMPTY;
        set_protocol(&mut protocol);
        assert_eq!(
            (
                protocol.package_length_size,
                protocol.package_length_offset,
                protocol.package_body_offset
            ),
            (4, 1, 0)
        );
        assert_eq!(
            protocol.get_package_length.map(|f| f as *const ()),
            Some(get_package_length as *const ())
        );
        let measure = |bytes: &[u8]| {
            let mut pl = PacketLength::new(bytes);
            pl.header_len = 99;
            let length = get_package_length(core::ptr::null(), core::ptr::null_mut(), &mut pl);
            assert_eq!(pl.header_len, 99);
            length
        };
        assert_eq!(measure(&[0x30]), 0);
        assert_eq!(measure(&[0x30, 0x80, 0]), 3);
        assert_eq!(measure(&[0x30, 0x80, 0x80, 0x80, 0x80]), -1);
    }
}
