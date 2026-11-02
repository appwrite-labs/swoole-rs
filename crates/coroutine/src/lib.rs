//! The coroutine scheduler and per-coroutine state, Channel queues and policy,
//! and context stack bookkeeping. C++ runs the effects: swap callbacks, stack
//! switches, timers and destructors.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

mod tokens;
pub use tokens::*;
mod channel;
pub use channel::*;
mod coroutine;
pub use coroutine::*;
mod stack;
pub use stack::*;
mod context;
pub use context::*;

#[repr(C)]
pub struct ChannelCursor {
    pub phase: u32,
}

const CLOSED: u32 = 1;
const EMPTY: u32 = 2;
const FULL: u32 = 4;
const PRODUCERS: u32 = 8;
const CONSUMERS: u32 = 16;
const POSITIVE_TIMEOUT: u32 = 32;
const HAS_TIMER: u32 = 64;
const CANCELED: u32 = 128;
const TIMEDOUT: u32 = 256;
const DONE: i32 = 0;
const ARM_TIMER: i32 = 1;
const YIELD: i32 = 2;
const DELETE: i32 = 3;
const TRANSFER: i32 = 4;
const WAKE_PRODUCER: i32 = 5;
const WAKE_CONSUMER: i32 = 6;
const PUBLISH_CLOSED: i32 = 7;
const FINISHED: u32 = 6;

/// # Safety
/// `cursor` is a valid, aligned, exclusively borrowed caller-local cursor. It
/// contains no native object pointers and is never retained after return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_channel_step(
    cursor: *mut ChannelCursor,
    op: u32,
    facts: u32,
) -> i32 {
    // SAFETY: `cursor` is null or live, aligned and exclusively lent for this call.
    let Some(cursor) = (unsafe { cursor.as_mut() }) else {
        return -4;
    };
    channel_step(cursor, op, facts)
}

fn channel_step(cursor: &mut ChannelCursor, op: u32, facts: u32) -> i32 {
    if op > 2 || cursor.phase > FINISHED {
        return -4;
    }
    if op == 2 {
        return match cursor.phase {
            0 => {
                if facts & CLOSED != 0 {
                    cursor.phase = FINISHED;
                    -2
                } else {
                    cursor.phase = 1;
                    PUBLISH_CLOSED
                }
            }
            1 => {
                if facts & PRODUCERS != 0 {
                    WAKE_PRODUCER
                } else if facts & CONSUMERS != 0 {
                    cursor.phase = 2;
                    WAKE_CONSUMER
                } else {
                    cursor.phase = FINISHED;
                    DONE
                }
            }
            2 => {
                // Never return to the producer loop once consumer wakeup began.
                if facts & CONSUMERS != 0 {
                    WAKE_CONSUMER
                } else {
                    cursor.phase = FINISHED;
                    DONE
                }
            }
            FINISHED => DONE,
            _ => -4,
        };
    }
    loop {
        match cursor.phase {
            0 => {
                if facts & CLOSED != 0 && (op == 0 || facts & EMPTY != 0) {
                    cursor.phase = FINISHED;
                    return -2;
                }
                let wait = if op == 0 {
                    facts & (FULL | PRODUCERS) != 0
                } else {
                    facts & (EMPTY | CONSUMERS) != 0
                };
                if wait {
                    cursor.phase = 1;
                    if facts & POSITIVE_TIMEOUT != 0 {
                        return ARM_TIMER;
                    }
                } else {
                    cursor.phase = 4;
                }
            }
            1 => {
                cursor.phase = 2;
                return YIELD;
            }
            2 => {
                cursor.phase = 3;
                if facts & HAS_TIMER != 0 {
                    return DELETE;
                }
            }
            3 => {
                let error = if facts & CANCELED != 0 {
                    -3
                } else if facts & TIMEDOUT != 0 {
                    -1
                } else if facts & CLOSED != 0 && (op == 0 || facts & EMPTY != 0) {
                    -2
                } else {
                    0
                };
                if error != 0 {
                    cursor.phase = FINISHED;
                    return error;
                }
                cursor.phase = 4;
            }
            4 => {
                cursor.phase = 5;
                return TRANSFER;
            }
            5 => {
                cursor.phase = FINISHED;
                if op == 0 && facts & CONSUMERS != 0 {
                    return WAKE_CONSUMER;
                }
                if op == 1 && facts & PRODUCERS != 0 {
                    return WAKE_PRODUCER;
                }
            }
            FINISHED => return DONE,
            _ => return -4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_fifo_and_cancel_precedence() {
        for (op, facts) in [(0, PRODUCERS), (1, CONSUMERS)] {
            let mut c = ChannelCursor { phase: 0 };
            unsafe {
                assert_eq!(swr_v1_channel_step(&raw mut c, op, facts), YIELD);
                assert_eq!(
                    swr_v1_channel_step(&raw mut c, op, HAS_TIMER | CANCELED | TIMEDOUT | CLOSED),
                    DELETE
                );
                assert_eq!(
                    swr_v1_channel_step(&raw mut c, op, CANCELED | TIMEDOUT | CLOSED),
                    -3
                );
            }
        }
    }
    #[test]
    fn close_finishes_producer_loop_before_consumer_loop() {
        let mut c = ChannelCursor { phase: 0 };
        unsafe {
            assert_eq!(swr_v1_channel_step(&raw mut c, 2, 0), PUBLISH_CLOSED);
            assert_eq!(
                swr_v1_channel_step(&raw mut c, 2, PRODUCERS | CONSUMERS),
                WAKE_PRODUCER
            );
            assert_eq!(swr_v1_channel_step(&raw mut c, 2, CONSUMERS), WAKE_CONSUMER);
            assert_eq!(swr_v1_channel_step(&raw mut c, 2, PRODUCERS), DONE);
        }
    }
}
