#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
// SPDX-License-Identifier: Apache-2.0
//! TLS and DTLS over OpenSSL (openssl-sys): `SSLContext`'s context creation and
//! configuration (`context`), `network::Socket`'s TLS I/O (`socket`, called from
//! reactor's patch), and the DTLS BIO's read/control/listen decisions.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
use core::ffi::c_long;

#[cfg(unix)]
mod context;
#[cfg(unix)]
pub mod reactor_write;
#[cfg(unix)]
mod socket;
#[cfg(unix)]
pub mod socket_pair;
#[cfg(unix)]
pub mod upstream;
#[cfg(unix)]
pub use context::*;
#[cfg(unix)]
pub use socket::*;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ReadPlan {
    pub length: i32,
    pub consume: u8,
    pub retry: u8,
}

/// Native callers only copy initialized datagrams with nonnegative requested length.
/// Negative lengths in the pinned BIO adapter lead to an invalid native memmove;
/// this scalar plan preserves the signed result without forming any Rust slice.
fn read_plan(empty: u8, front: u16, requested: i32, peek: u8) -> ReadPlan {
    if empty != 0 {
        ReadPlan {
            length: -1,
            consume: 0,
            retry: 1,
        }
    } else {
        ReadPlan {
            length: i32::from(front).min(requested),
            consume: u8::from(peek == 0),
            retry: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ControlPlan {
    pub result: c_long,
    pub effect: u32,
}

// Tags are decoded from the actual compiled OpenSSL macros by native code.
// Effects: 0 none, 1 set shutdown, 2 copy peer, 3 set peek, 4 warn unknown.
fn control_plan(tag: u32, observed: c_long) -> ControlPlan {
    let (result, effect) = match tag {
        0..=2 => (observed, 0), // EOF, shutdown, total queued bytes
        3 => (0, 1),
        4 => (1, 0),
        5 => (1, 2),
        6 => (1500, 0),
        7 => (96, 0),
        8 => (1, 3),
        9 => (0, 0),
        _ => (0, 4),
    };
    ControlPlan { result, effect }
}

/// 0: failure/warn, 1: pending success, 2: success and publish listened.
fn listen_plan(result: i32) -> u32 {
    match result.cmp(&0) {
        core::cmp::Ordering::Less => 0,
        core::cmp::Ordering::Equal => 1,
        core::cmp::Ordering::Greater => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_short_and_peek_datagrams() {
        for front in [0, 1, 255, 65535] {
            for requested in [0, 1, 256, 65536, i32::MAX] {
                for peek in [0, 1] {
                    let p = read_plan(0, front, requested, peek);
                    assert_eq!(p.length, i32::from(front).min(requested));
                    assert_eq!(p.consume, u8::from(peek == 0));
                    assert_eq!(p.retry, 0);
                }
            }
        }
        assert_eq!(
            read_plan(1, 0, 0, 0),
            ReadPlan {
                length: -1,
                consume: 0,
                retry: 1
            }
        );
    }
    #[test]
    fn control_observations_and_effects() {
        for value in [c_long::MIN, -1, 0, 1, c_long::MAX] {
            for tag in 0..=2 {
                assert_eq!(control_plan(tag, value).result, value);
            }
        }
        assert_eq!(
            control_plan(3, 99),
            ControlPlan {
                result: 0,
                effect: 1
            }
        );
        assert_eq!(
            control_plan(5, 0),
            ControlPlan {
                result: 1,
                effect: 2
            }
        );
        assert_eq!(
            control_plan(8, 0),
            ControlPlan {
                result: 1,
                effect: 3
            }
        );
        assert_eq!(
            control_plan(u32::MAX, 0),
            ControlPlan {
                result: 0,
                effect: 4
            }
        );
    }
    #[test]
    fn listen_pending_is_success_without_publication() {
        for x in [i32::MIN, -1] {
            assert_eq!(listen_plan(x), 0);
        }
        assert_eq!(listen_plan(0), 1);
        for x in [1, i32::MAX] {
            assert_eq!(listen_plan(x), 2);
        }
    }
}

#[cfg(unix)]
mod owned;
#[cfg(unix)]
pub use owned::*;

#[cfg(unix)]
mod info;

mod constants;
pub use constants::*;

#[cfg(unix)]
mod dtls;
#[cfg(unix)]
pub use dtls::*;
