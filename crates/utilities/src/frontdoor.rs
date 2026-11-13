//! Whole logging-frontdoor programs. All observations/effects happen after return.
use swoole_rs_corebase::CCast;
#[repr(C)]
#[derive(Default)]
pub struct SwrLogFrontdoor {
    pub phase: u32,
    pub admitted: u32,
}
#[repr(C)]
pub struct SwrLogAction {
    pub action: u32,
    pub level: i32,
}

/// Advance one complete macro program with facts from the requested effect only.
/// Operations: info, notice, warning, debug, trace, `sys_notice`, `sys_warning`,
/// `error_log`, error, `sys_error`. Actions are declared in the matching C header.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v2_utils_log_frontdoor_step(
    state: &mut SwrLogFrontdoor,
    operation: u32,
    first: i64,
    second: i64,
    unsigned_width: u32,
    high: u64,
) -> SwrLogAction {
    let level = match operation {
        0 => 2,
        1 | 5 => 3,
        2 | 6 => 4,
        3 => 0,
        4 => 1,
        7..=9 => 5,
        _ => {
            return SwrLogAction {
                action: 0,
                level: 0,
            };
        }
    };
    let action = match state.phase {
        0 if matches!(operation, 5..=7) => {
            state.phase = 1;
            1
        }
        0 if operation >= 8 => {
            state.admitted = 1;
            state.phase = 4;
            5
        }
        0 | 1 => {
            state.phase = 2;
            2
        }
        2 => {
            let admission = match operation {
                7 => first,
                5 | 6 => 5,
                _ => i64::from(level),
            };
            let accepted = if operation == 7 {
                match unsigned_width {
                    32 => first.c_cast::<u32>() >= second.c_cast::<u32>(),
                    64 => first.c_cast::<u64>() >= second.c_cast::<u64>(),
                    128 => {
                        ((u128::from(high) << 64) | u128::from(first.c_cast::<u64>()))
                            >= second.c_cast::<u128>()
                    }
                    129 => {
                        ((u128::from(high) << 64) | u128::from(first.c_cast::<u64>()))
                            .c_cast::<i128>()
                            >= i128::from(second)
                    }
                    256 | 257 => first != 0,
                    _ => first >= second,
                }
            } else {
                admission >= second
            };
            if !accepted {
                state.phase = 7;
                0
            } else if operation == 4 {
                state.phase = 3;
                3
            } else if operation == 7 && unsigned_width == 257 {
                state.admitted = 1;
                state.phase = 4;
                5
            } else if operation == 7 {
                state.phase = 8;
                4
            } else {
                state.admitted = 1;
                state.phase = 4;
                5
            }
        }
        3 if (first.c_cast::<u64>() & second.c_cast::<u64>()) == 0 => {
            state.phase = 7;
            0
        }
        8 if first != 0 => {
            state.phase = 7;
            0
        }
        3 | 8 => {
            state.admitted = 1;
            state.phase = 4;
            5
        }
        4 => {
            state.phase = 5;
            6
        }
        5 if operation >= 8 => {
            state.phase = 6;
            7
        }
        _ => {
            state.phase = 7;
            0
        }
    };
    SwrLogAction { action, level }
}

#[repr(C)]
#[derive(Default)]
pub struct SwrLogHex {
    pub phase: u32,
    pub offset: usize,
    pub index: usize,
    pub hex_length: usize,
    pub text_length: usize,
}

/// Build the complete hex-record program, requesting original native sprintf,
/// byte observations and ctype effects individually. No input pointer is retained.
/// Caller-owned row buffers are written only by the requested native effects.
/// `length` remains the original captured length. `previous` is the last requested
/// effect result.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v2_utils_log_hex_step(
    s: &mut SwrLogHex,
    length: usize,
    previous: i32,
) -> u32 {
    let next_byte = |s: &mut SwrLogHex| {
        if s.index < s.offset.wrapping_add(16) && s.index < length {
            s.phase = 2;
            2 // hex sprintf
        } else {
            s.phase = 5;
            6 // complete row emission
        }
    };
    let next_row = |s: &mut SwrLogHex| {
        if s.offset < length {
            s.index = s.offset;
            s.hex_length = 0;
            s.text_length = 0;
            next_byte(s)
        } else {
            s.phase = 6;
            7 // footer
        }
    };
    match s.phase {
        0 => {
            s.offset = 0;
            s.phase = 1;
            1
        } // header
        1 => next_row(s),
        2 => {
            s.hex_length = s.hex_length.wrapping_add(previous.c_cast::<usize>());
            s.phase = 3;
            3
        }
        3 => {
            s.phase = 4;
            if previous != 0 { 4 } else { 5 }
        }
        4 => {
            s.text_length = s.text_length.wrapping_add(previous.c_cast::<usize>());
            s.index = s.index.wrapping_add(1);
            next_byte(s)
        }
        5 => {
            s.offset = s.offset.wrapping_add(16);
            next_row(s)
        }
        _ => {
            s.phase = 7;
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn program(op: u32, threshold: i64, ignored: bool, mask: i64) -> Vec<u32> {
        let mut state = SwrLogFrontdoor::default();
        let (mut a, mut b) = (0, 0);
        let mut trace = Vec::new();
        loop {
            let action = swr_v2_utils_log_frontdoor_step(&mut state, op, a, b, 0, 0).action;
            if action == 0 {
                return trace;
            }
            trace.push(action);
            match action {
                2 => {
                    a = 4;
                    b = threshold;
                }
                3 => {
                    a = mask;
                    b = 1;
                }
                4 => a = i64::from(ignored),
                _ => (),
            }
        }
    }
    #[test]
    fn complete_effect_traces() {
        assert_eq!(program(5, 6, false, 0), [1, 2]);
        assert_eq!(program(5, 5, false, 0), [1, 2, 5, 6]);
        assert_eq!(program(7, 6, false, 0), [1, 2]);
        assert_eq!(program(7, 4, true, 0), [1, 2, 4]);
        assert_eq!(program(7, 4, false, 0), [1, 2, 4, 5, 6]);
        assert_eq!(program(4, 2, false, 1), [2]);
        assert_eq!(program(4, 1, false, 0), [2, 3]);
        assert_eq!(program(4, 1, false, 1), [2, 3, 5, 6]);
        // Pure program trace only, no actual native exit/failure probe.
        assert_eq!(program(8, 6, false, 0), [5, 6, 7]);
    }
    #[test]
    fn hex_rows_and_native_effect_boundaries() {
        for length in [0, 1, 15, 16, 17, 31, 32, 33] {
            let mut state = SwrLogHex::default();
            let (mut previous, mut rows, mut bytes, mut headers, mut footers) = (0, 0, 0, 0, 0);
            loop {
                let action = swr_v2_utils_log_hex_step(&mut state, length, previous);
                match action {
                    0 => break,
                    1 => headers += 1,
                    2 => {
                        bytes += 1;
                        previous = 3;
                    }
                    3 => previous = i32::from(state.index % 2 == 0),
                    4 | 5 => previous = 1,
                    6 => {
                        rows += 1;
                        assert!(state.hex_length <= 48 && state.text_length <= 16);
                    }
                    7 => footers += 1,
                    _ => panic!("unexpected action"),
                }
            }
            assert_eq!(
                (headers, footers, bytes, rows),
                (1, 1, length, length.div_ceil(16))
            );
        }
    }
}
