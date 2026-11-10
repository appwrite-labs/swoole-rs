//! Method bodies shared by Atomic and Thread\Atomic: each parses its arguments
//! as upstream's ZPP block does, applies one locks-crate operation to the
//! object's word and sets the return value.
use swoole_rs_locks::{atomic32, atomic64};
use swoole_rs_zend::Call;

const LOAD: u32 = 0;
pub(crate) const STORE: u32 = 1;
const ADD_FETCH: u32 = 2;
const SUB_FETCH: u32 = 3;
const COMPARE_SET: u32 = 4;

/// A shared word: `sw_atomic_t` (u32) or `sw_atomic_long_t` (i64).
pub(crate) trait Word: Sized {
    /// The locks crate's `atomic32`/`atomic64`, with PHP ints cast to the word as upstream's
    /// `(uint32_t)` and `(sw_atomic_long_t)` casts do.
    /// # Safety
    /// `word` is null or a live word accessed only atomically.
    unsafe fn apply(word: *mut Self, op: u32, value: i64, expected: i64) -> i64;
}

impl Word for u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream casts zend_long to uint32_t"
    )]
    unsafe fn apply(word: *mut Self, op: u32, value: i64, expected: i64) -> i64 {
        // SAFETY: per the caller; the locks crate rejects null and misalignment.
        i64::from(unsafe { atomic32(word, op, value as u32, expected as u32) })
    }
}

impl Word for i64 {
    unsafe fn apply(word: *mut Self, op: u32, value: i64, expected: i64) -> i64 {
        // SAFETY: per the caller; the locks crate rejects null and misalignment.
        unsafe { atomic64(word, op, value, expected) }
    }
}

// Every body takes the word of `$this`, which its handler located; the word is
// null or live and atomic-only, as `Word::apply` requires.

/// `__construct(int $value = 0)`: store the initial value; false when the
/// arguments do not parse.
pub(crate) fn construct<W: Word>(call: &Call, word: *mut W) -> bool {
    let Ok(value) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return false;
    };
    // SAFETY: see above.
    unsafe { W::apply(word, STORE, value, 0) };
    true
}

/// `add(int $add_value = 1): int`.
pub(crate) fn add<W: Word>(call: &Call, word: *mut W) {
    fetch(call, word, ADD_FETCH);
}

/// `sub(int $sub_value = 1): int`.
pub(crate) fn sub<W: Word>(call: &Call, word: *mut W) {
    fetch(call, word, SUB_FETCH);
}

/// `ZEND_PARSE_PARAMETERS_END_EX(RETURN_FALSE)`, then raise a bailout a user
/// error handler left in a coercion (the hot path skips the check; no frame
/// here owns anything that needs a drop).
#[cold]
fn failed(call: &Call) {
    call.set_bool(false);
    swoole_rs_zend::bailout::finish();
}

fn fetch<W: Word>(call: &Call, word: *mut W, op: u32) {
    match call.arity(0, 1).and_then(|()| call.long(1, 1)) {
        // SAFETY: see above.
        Ok(value) => call.set_long(unsafe { W::apply(word, op, value, 0) }),
        Err(_) => failed(call),
    }
}

/// `get(): int`.
pub(crate) fn get<W: Word>(call: &Call, word: *mut W) {
    // SAFETY: see above.
    call.set_long(unsafe { W::apply(word, LOAD, 0, 0) });
}

/// `set(int $value): void`.
pub(crate) fn set<W: Word>(call: &Call, word: *mut W) {
    match call.arity(1, 1).and_then(|()| call.long(1, 0)) {
        Ok(value) => {
            // SAFETY: see above.
            unsafe { W::apply(word, STORE, value, 0) };
        }
        Err(_) => failed(call),
    }
}

/// `cmpset(int $cmp_value, int $new_value): bool`.
pub(crate) fn cmpset<W: Word>(call: &Call, word: *mut W) {
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 0)?)));
    match parsed {
        Ok((expected, value)) => {
            // SAFETY: see above.
            let swapped = unsafe { W::apply(word, COMPARE_SET, value, expected) };
            call.set_bool(swapped != 0);
        }
        Err(_) => failed(call),
    }
}

/// `wait(float $timeout = 1.0): bool`.
pub(crate) fn wait(call: &Call, word: *mut u32) {
    match call.arity(0, 1).and_then(|()| call.double(1, 1.0)) {
        // SAFETY: see above.
        Ok(timeout) => call.set_bool(unsafe { futex::wait(word, timeout) } >= 0),
        Err(_) => failed(call),
    }
}

/// `wakeup(int $count = 1): bool`.
pub(crate) fn wakeup(call: &Call, word: *mut u32) {
    match call.arity(0, 1).and_then(|()| call.long(1, 1)) {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream casts zend_long to int"
        )]
        // SAFETY: see above.
        Ok(count) => call.set_bool(unsafe { futex::wakeup(word, count as i32) } >= 0),
        Err(_) => failed(call),
    }
}

#[cfg(target_os = "linux")]
mod futex {
    pub(super) use swoole_rs_locks::{swr_v1_locks_wait as wait, swr_v1_locks_wakeup as wakeup};
}

/// Without futexes, crates/utilities' wait registry (upstream's `src/core/misc.cc`).
#[cfg(not(target_os = "linux"))]
mod futex {
    pub(super) use swoole_rs_utilities::misc::{
        sw_atomic_futex_wait as wait, sw_atomic_futex_wakeup as wakeup,
    };
}

/// Export Zend method handlers (`zim_<class>_<method>`, so upstream's `PHP_ME`
/// tables name them unchanged) that locate the word of `$this` and run a body.
macro_rules! methods {
    ($locate:ident; $($name:ident: $word:ty => $body:expr;)*) => {$(
        /// A Zend method handler.
        /// # Safety
        /// Called by Zend with a live frame and return slot.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            frame: *mut swoole_rs_zend::sys::zend_execute_data,
            value: *mut swoole_rs_zend::sys::zval,
        ) {
            // SAFETY: per Zend's handler contract.
            let call = unsafe { swoole_rs_zend::Call::new(frame, value) };
            let word: *mut $word = $locate(&call);
            $body(&call, word);
        }
    )*};
}
pub(crate) use methods;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_wrap_and_truncate_like_upstream() {
        let mut word = u32::MAX;
        // SAFETY: a live local word.
        let sum = unsafe { u32::apply(&raw mut word, ADD_FETCH, 2, 0) };
        assert_eq!(sum, 1, "32-bit add wraps");
        // SAFETY: as above.
        unsafe { u32::apply(&raw mut word, STORE, -1, 0) };
        assert_eq!(word, u32::MAX, "store truncates the PHP int");
        // SAFETY: as above.
        let swapped = unsafe { u32::apply(&raw mut word, COMPARE_SET, 7, -1) };
        assert_eq!((swapped, word), (1, 7), "CAS compares the truncated value");
        let mut long = i64::MAX;
        // SAFETY: a live local word.
        let sum = unsafe { i64::apply(&raw mut long, ADD_FETCH, 1, 0) };
        assert_eq!(sum, i64::MIN, "64-bit add wraps");
    }
}
