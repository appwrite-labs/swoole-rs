//! Common PHP helper policy. Zend storage, calls, allocation and VM effects stay native.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
mod asynchronous;
mod facade;
mod known_strings;

/// Empty string/array predicate over a count observed by the native Zend shim.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phphelpers_empty(count: usize) -> u8 {
    u8::from(count == 0)
}

fn merge_action(phase: &mut u32, inserted: u8, existing_array: u8) -> u32 {
    let (action, next) = match *phase {
        0 => (1, 1),
        1 if inserted != 0 => (0, 3),
        1 => (2, 2),
        2 => (if existing_array != 0 { 3 } else { 4 }, 3),
        _ => (0, 3),
    };
    *phase = next;
    action
}

/// Keyed variant of the frozen merge lifecycle. Numeric-key classification stays
/// native; Rust selects concrete `HashTable` effects without an extra dispatch.
/// # Safety
/// Same phase storage/lazy observation requirements as `swr_v1_phphelpers_merge`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphelpers_merge_keyed(
    phase: *mut u32,
    inserted: u8,
    existing_array: u8,
    numeric_key: u8,
) -> u32 {
    // SAFETY: `phase` is null or the caller's aligned, exclusively owned phase storage.
    let Some(phase) = (unsafe { phase.as_mut() }) else {
        return 0;
    };
    match merge_action(phase, inserted, existing_array) {
        1 => {
            if numeric_key != 0 {
                1
            } else {
                2
            }
        }
        2 => {
            if numeric_key != 0 {
                3
            } else {
                4
            }
        }
        3 => 5,
        4 => {
            if numeric_key != 0 {
                6
            } else {
                7
            }
        }
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phphelpers_call_success(result: i64, coroutine: u8) -> u8 {
    call_success(result, coroutine)
}

fn call_success(result: i64, coroutine: u8) -> u8 {
    u8::from(if coroutine != 0 {
        result >= 0
    } else {
        result == 0
    })
}

#[repr(u32)]
enum CallAction {
    Null = 0,
    Direct = 1,
    Coroutine = 2,
    Named = 3,
    ReleaseName = 4,
    ObserveException = 5,
    Fatal = 6,
    True = 7,
    False = 8,
    Variable = 9,
}

/// # Safety
/// Phase is aligned native-owned storage initialized to0. Mode is direct0,
/// coroutine1 or named2. Result and exception are fresh observations, consumed
/// only after their native call/effect; no engine pointer crosses this interface.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphelpers_call_plan(
    phase: *mut u32,
    mode: u32,
    has_retval: u8,
    result: i64,
    exception: u8,
) -> u32 {
    use CallAction::{
        Coroutine, Direct, False, Fatal, Named, Null, ObserveException, ReleaseName, True, Variable,
    };
    // SAFETY: `phase` is null or the caller's aligned, exclusively owned phase storage.
    let Some(phase) = (unsafe { phase.as_mut() }) else {
        return False as u32;
    };
    let invoke = match mode {
        0 => Direct,
        1 => Coroutine,
        _ => Named,
    };
    let (action, next) = match *phase {
        0 if mode == 1 && has_retval != 0 => (Null, 1),
        0 | 1 => (invoke, 2),
        2 if mode == 2 && result != 0 => (Null, 3),
        2 if mode == 2 => (ReleaseName, 4),
        2 | 4 => (ObserveException, 5),
        3 => (ReleaseName, 4),
        5 if exception != 0 => (Fatal, 6),
        _ if mode == 2 => (Variable, 6),
        _ if call_success(result, u8::from(mode == 1)) != 0 => (True, 6),
        _ => (False, 6),
    };
    *phase = next;
    action as u32
}

/// # Safety
/// Phase is native-owned aligned storage initialized to0, no retained pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphelpers_callable(phase: *mut u32, truthy: u8, valid: u8) -> u32 {
    // SAFETY: `phase` is null or the caller's aligned, exclusively owned phase storage.
    let Some(phase) = (unsafe { phase.as_mut() }) else {
        return 0;
    };
    let (action, next) = match *phase {
        0 => (1, 1),
        1 if truthy == 0 => (3, 3),
        1 => (2, 2),
        2 => (if valid != 0 { 5 } else { 4 }, 3),
        _ => (0, 3),
    };
    *phase = next;
    action
}

/// # Safety
/// Phase is aligned native-owned storage initialized to0. Observation is fresh
/// value-defined/name-present fact, with no borrowed Zend pointer retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphelpers_release(phase: *mut u32, observation: u8) -> u32 {
    // SAFETY: `phase` is null or the caller's aligned, exclusively owned phase storage.
    let Some(phase) = (unsafe { phase.as_mut() }) else {
        return 0;
    };
    let (action, next) = match *phase {
        0 => (1, 1),
        1 if observation != 0 => (2, 2),
        1 | 2 => (3, 3),
        3 if observation != 0 => (4, 4),
        _ => (0, 4),
    };
    *phase = next;
    action
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callable_observation_order_is_lazy() {
        for truthy in [0, 1] {
            for valid in [0, 1] {
                let mut phase = 0;
                assert_eq!(
                    unsafe { swr_v1_phphelpers_callable(&raw mut phase, truthy, valid) },
                    1
                );
                let next = unsafe { swr_v1_phphelpers_callable(&raw mut phase, truthy, valid) };
                if truthy == 0 {
                    assert_eq!(next, 3);
                } else {
                    assert_eq!(next, 2);
                    assert_eq!(
                        unsafe { swr_v1_phphelpers_callable(&raw mut phase, truthy, valid) },
                        if valid == 0 { 4 } else { 5 }
                    );
                }
                assert_eq!(
                    unsafe { swr_v1_phphelpers_callable(&raw mut phase, truthy, valid) },
                    0
                );
            }
        }
    }
    #[test]
    fn call_success_follows_the_call_mode() {
        for result in [-1, 0, 1, i64::MAX] {
            assert_eq!(
                swr_v1_phphelpers_call_success(result, 0),
                u8::from(result == 0)
            );
            assert_eq!(
                swr_v1_phphelpers_call_success(result, 1),
                u8::from(result >= 0)
            );
        }
    }
    #[test]
    fn shared_common_policy_preserves_positive_lifecycles() {
        for numeric in [0, 1] {
            for existing in [0, 1] {
                let mut phase = 0;
                let first =
                    unsafe { swr_v1_phphelpers_merge_keyed(&raw mut phase, 0, existing, numeric) };
                assert_eq!(first, if numeric != 0 { 1 } else { 2 });
                let next =
                    unsafe { swr_v1_phphelpers_merge_keyed(&raw mut phase, 0, existing, numeric) };
                assert_eq!(next, if numeric != 0 { 3 } else { 4 });
                let final_action =
                    unsafe { swr_v1_phphelpers_merge_keyed(&raw mut phase, 0, existing, numeric) };
                assert_eq!(
                    final_action,
                    if existing != 0 {
                        5
                    } else if numeric != 0 {
                        6
                    } else {
                        7
                    }
                );
                assert_eq!(
                    unsafe { swr_v1_phphelpers_merge_keyed(&raw mut phase, 0, existing, numeric) },
                    0
                );
            }
        }
    }
}
