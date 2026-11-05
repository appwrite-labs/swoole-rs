//! Coroutine policy and sole channel token storage. Native code owns payloads
//! and executes callbacks, stack switches, timers and destructors after return.

#[path = "../../storage.rs"]
mod storage;
mod tokens;
pub use tokens::*;
mod channel;
pub use channel::*;
mod scheduler;
pub use scheduler::*;
mod timing;
pub use timing::*;
mod stack;
pub use stack::*;
mod status;
pub use status::*;
mod context;
pub use context::*;
mod identity;
pub use identity::*;
mod swap;
pub use swap::*;
mod creation;
pub use creation::*;
mod bailout;
pub use bailout::*;
mod entry;
pub use entry::*;

const PROCEED: u32 = 1;
const CALL: u32 = 2;
const CLEAR_CANCEL: u32 = 4;
const DELETE_TIMER: u32 = 8;
const DESTROY: u32 = 16;
const CALL_BAILOUT: u32 = 32;
const BAILOUT: u32 = 1;
const CALLBACK: u32 = 2;
const TASK: u32 = 4;
const CANCELABLE: u32 = 8;
const ENDED: u32 = 16;
const TIMER: u32 = 32;

#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub actions: u32,
    /// -1 preserves the native state/code; other values are native enum values.
    pub state: i32,
    pub resume_code: i32,
    /// 0 success; -1 timeout, -2 canceled, -3 cannot cancel, -4 invalid operation.
    pub error: i32,
}

/// Pure copied-fact decisions. Native execution order is part of the facade's
/// contract; in particular cancel copies its native function BEFORE applying
/// this plan and never accesses the object after invoking that function.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_coroutine_transition(op: u32, facts: u32, code: i32) -> Plan {
    coroutine_transition(op, facts, code)
}

fn coroutine_transition(op: u32, facts: u32, code: i32) -> Plan {
    let mut plan = Plan {
        actions: 0,
        state: -1,
        resume_code: i32::MIN,
        error: 0,
    };
    match op {
        0 | 1 | 2 | 4 => {
            if op == 1 && facts & BAILOUT != 0 {
                return plan;
            }
            plan.actions = PROCEED;
            plan.state = match op {
                2 => 1,
                4 => 3,
                _ => 2,
            };
            if op == 2 {
                plan.resume_code = 0;
            }
            if op != 0 && facts & (CALLBACK | TASK) == CALLBACK | TASK {
                plan.actions |= CALL;
            }
        }
        3 => {
            if facts & CANCELABLE == 0 {
                plan.error = -3;
            } else {
                plan.actions = PROCEED | CLEAR_CANCEL;
                plan.resume_code = -2;
            }
        }
        5 => {
            plan.actions = if facts & ENDED != 0 {
                DESTROY
            } else if facts & BAILOUT != 0 {
                CALL_BAILOUT
            } else {
                0
            };
        }
        6 => {
            if code == -1 {
                plan.error = -1;
            } else if facts & TIMER != 0 {
                plan.actions = DELETE_TIMER;
            }
        }
        7 => {
            if code == -2 {
                plan.error = -2;
            }
        }
        _ => plan.error = -4,
    }
    plan
}

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
    fn scheduler_precedence() {
        assert_eq!(
            swr_v1_coroutine_transition(1, BAILOUT | CALLBACK | TASK, 0).actions,
            0
        );
        assert_eq!(
            swr_v1_coroutine_transition(5, ENDED | BAILOUT, 0).actions,
            DESTROY
        );
        assert_eq!(swr_v1_coroutine_transition(6, TIMER, -1).actions, 0);
        assert_eq!(
            swr_v1_coroutine_transition(6, TIMER, -2).actions,
            DELETE_TIMER
        );
        assert_eq!(swr_v1_coroutine_transition(7, 0, -2).error, -2);
    }
    #[test]
    fn channel_fifo_and_cancel_precedence() {
        for (op, facts) in [(0, PRODUCERS), (1, CONSUMERS)] {
            let mut c = ChannelCursor { phase: 0 };
            unsafe {
                assert_eq!(swr_v1_channel_step(&mut c, op, facts), YIELD);
                assert_eq!(
                    swr_v1_channel_step(&mut c, op, HAS_TIMER | CANCELED | TIMEDOUT | CLOSED),
                    DELETE
                );
                assert_eq!(
                    swr_v1_channel_step(&mut c, op, CANCELED | TIMEDOUT | CLOSED),
                    -3
                );
            }
        }
    }
    #[test]
    fn close_finishes_producer_loop_before_consumer_loop() {
        let mut c = ChannelCursor { phase: 0 };
        unsafe {
            assert_eq!(swr_v1_channel_step(&mut c, 2, 0), PUBLISH_CLOSED);
            assert_eq!(
                swr_v1_channel_step(&mut c, 2, PRODUCERS | CONSUMERS),
                WAKE_PRODUCER
            );
            assert_eq!(swr_v1_channel_step(&mut c, 2, CONSUMERS), WAKE_CONSUMER);
            assert_eq!(swr_v1_channel_step(&mut c, 2, PRODUCERS), DONE);
        }
    }
}

mod registry;
pub use registry::*;
