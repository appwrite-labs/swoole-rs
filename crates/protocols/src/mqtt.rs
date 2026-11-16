/// MQTT's remaining length continues beyond its four-byte limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemainingLengthTooLong;

/// Parse MQTT remaining-length bytes; packet type and nonminimal representations
/// are intentionally not validated, matching the pinned protocol. `None` means
/// the length header is incomplete; a complete header yields the whole packet size.
///
/// # Errors
/// [`RemainingLengthTooLong`] when the fourth length byte still has its continuation bit set.
pub fn mqtt_frame_length(bytes: &[u8]) -> Result<Option<usize>, RemainingLengthTooLong> {
    let mut length = 0;
    for (position, byte) in (0usize..).zip(bytes.iter().skip(1).take(4)) {
        length += usize::from(byte & 127) << (7 * position);
        if byte & 128 == 0 {
            return Ok(Some(length + position + 2));
        }
        if position == 3 {
            return Err(RemainingLengthTooLong);
        }
    }
    Ok(None)
}

#[cfg(test)]
#[expect(
    clippy::cast_possible_wrap,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests {
    // Original integer expectations stay at this test boundary.
    fn mqtt_frame_length(bytes: &[u8]) -> i64 {
        super::mqtt_frame_length(bytes).map_or(-1, |length| {
            length.map_or(0, |n| i64::try_from(n).unwrap_or(-1))
        })
    }
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
}
