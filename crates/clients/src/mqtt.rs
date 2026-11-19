//! `swoole::mqtt`'s entries, over [`mqtt_frame_length`].
use crate::protocol::Protocol;
use swoole_rs_corebase::net_types::PacketLength;
use swoole_rs_error::ErrorCode::SW_ERROR_PACKAGE_LENGTH_TOO_LARGE;
use swoole_rs_protocols::{MQTT_MAX_LENGTH_SIZE as MAX_LENGTH_SIZE, mqtt_frame_length};

/// `swoole::mqtt::set_protocol(protocol)`.
pub fn set_protocol(protocol: &mut Protocol) {
    protocol.package_length_size = MAX_LENGTH_SIZE;
    protocol.package_length_offset = 1;
    protocol.package_body_offset = 0;
    protocol.get_package_length = Some(get_package_length);
}

/// `swoole::mqtt::get_package_length(protocol, socket, pl)`, the protocol's length
/// callback: 0 until the remaining length is buffered, then the whole packet's length;
/// `None`, with a warning, past four length bytes. It reads only `pl`.
pub fn get_package_length(
    _: &Protocol,
    _: Option<u8>,
    _: Option<std::rc::Rc<dyn core::any::Any>>,
    pl: &mut PacketLength,
) -> Option<usize> {
    if let Ok(length) = mqtt_frame_length(pl.bytes()) {
        Some(length.unwrap_or(0))
    } else {
        swoole_rs_corebase::log::error_log(
            swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING,
            SW_ERROR_PACKAGE_LENGTH_TOO_LARGE as i32,
            c"mqtt::get_package_length",
            format_args!("bad request, the variable header size is larger than {MAX_LENGTH_SIZE}"),
        );
        None
    }
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
        // The stored callback measures, rather than comparing addresses: a fn's address
        // isn't unique (Miri gives each use its own).
        let stored = protocol
            .get_package_length
            .expect("set_protocol stores the callback");
        let measure = |bytes: &[u8]| {
            let mut pl = PacketLength::new(bytes);
            pl.header_len = 99;
            let length = stored(&Protocol::EMPTY, None, None, &mut pl);
            assert_eq!(pl.header_len, 99);
            length
        };
        assert_eq!(measure(&[0x30]), Some(0));
        assert_eq!(measure(&[0x30, 0x80, 0]), Some(3));
        assert_eq!(measure(&[0x30, 0x80, 0x80, 0x80, 0x80]), None);
    }
}
