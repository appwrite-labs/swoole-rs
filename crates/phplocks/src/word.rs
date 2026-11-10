//! Method bodies shared by Atomic and Thread\Atomic: each parses its arguments
//! as upstream's ZPP block does, applies one atomic operation to the object's
//! word and sets the return value.
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use swoole_rs_utilities::misc::{futex_wait, futex_wakeup};
use swoole_rs_zend::{Call, Custom};

/// A shared word: `sw_atomic_t` (u32) or `sw_atomic_long_t` (i64), with PHP
/// ints cast to it as upstream's `(uint32_t)` and `(sw_atomic_long_t)` casts
/// do, and the `__sync` builtins' wrapping and fences.
pub(crate) trait Word {
    fn get(&self) -> i64;
    fn set(&self, value: i64);
    fn add(&self, value: i64) -> i64;
    fn sub(&self, value: i64) -> i64;
    fn cmpset(&self, expected: i64, value: i64) -> bool;
}

macro_rules! word {
    ($atomic:ty, $cast:expr) => {
        impl Word for $atomic {
            fn get(&self) -> i64 {
                self.load(Ordering::Relaxed).into()
            }
            fn set(&self, value: i64) {
                self.store($cast(value), Ordering::Relaxed);
            }
            fn add(&self, value: i64) -> i64 {
                let value = $cast(value);
                self.fetch_add(value, Ordering::SeqCst)
                    .wrapping_add(value)
                    .into()
            }
            fn sub(&self, value: i64) -> i64 {
                let value = $cast(value);
                self.fetch_sub(value, Ordering::SeqCst)
                    .wrapping_sub(value)
                    .into()
            }
            fn cmpset(&self, expected: i64, value: i64) -> bool {
                self.compare_exchange(
                    $cast(expected),
                    $cast(value),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            }
        }
    };
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "upstream casts zend_long to uint32_t"
)]
const fn word32(value: i64) -> u32 {
    value as u32
}

word!(AtomicU32, word32);
word!(AtomicI64, std::convert::identity);

/// An object whose methods apply to a word: none (upstream's null) when the
/// pool had none, or before a thread atomic's constructor.
pub(crate) trait Holder: Custom {
    type Word: Word;
    fn word(&self) -> Option<&Self::Word>;
}

fn word<H: Holder>(call: &Call) -> Option<&H::Word> {
    call.custom::<H>()?.word()
}

/// `__construct(int $value = 0)`: store the initial value; false when the
/// arguments do not parse.
pub(crate) fn construct<H: Holder>(call: &Call) -> bool {
    let Ok(value) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return false;
    };
    if let Some(word) = word::<H>(call) {
        word.set(value);
    }
    true
}

/// `add(int $add_value = 1): int`.
pub(crate) fn add<H: Holder>(call: &Call) {
    fetch::<H>(call, Word::add);
}

/// `sub(int $sub_value = 1): int`.
pub(crate) fn sub<H: Holder>(call: &Call) {
    fetch::<H>(call, Word::sub);
}

/// `ZEND_PARSE_PARAMETERS_END_EX(RETURN_FALSE)`, then raise a bailout a user
/// error handler left in a coercion (the hot path, a leaf handler, skips the
/// check; no frame here owns anything that needs a drop).
#[cold]
fn failed(call: &Call) {
    call.set_bool(false);
    swoole_rs_zend::bailout::finish();
}

fn fetch<H: Holder>(call: &Call, op: fn(&H::Word, i64) -> i64) {
    match call.arity(0, 1).and_then(|()| call.long(1, 1)) {
        Ok(value) => call.set_long(word::<H>(call).map_or(0, |word| op(word, value))),
        Err(_) => failed(call),
    }
}

/// `get(): int`.
pub(crate) fn get<H: Holder>(call: &Call) {
    call.set_long(word::<H>(call).map_or(0, Word::get));
}

/// `set(int $value): void`.
pub(crate) fn set<H: Holder>(call: &Call) {
    match call.arity(1, 1).and_then(|()| call.long(1, 0)) {
        Ok(value) => {
            if let Some(word) = word::<H>(call) {
                word.set(value);
            }
        }
        Err(_) => failed(call),
    }
}

/// `cmpset(int $cmp_value, int $new_value): bool`.
pub(crate) fn cmpset<H: Holder>(call: &Call) {
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 0)?)));
    match parsed {
        Ok((expected, value)) => {
            call.set_bool(word::<H>(call).is_some_and(|word| word.cmpset(expected, value)));
        }
        Err(_) => failed(call),
    }
}

/// `wait(float $timeout = 1.0): bool`.
pub(crate) fn wait<H: Holder<Word = AtomicU32>>(call: &Call) {
    match call.arity(0, 1).and_then(|()| call.double(1, 1.0)) {
        Ok(timeout) => {
            call.set_bool(word::<H>(call).is_some_and(|word| futex_wait(word, timeout) >= 0));
        }
        Err(_) => failed(call),
    }
}

/// `wakeup(int $count = 1): bool`.
pub(crate) fn wakeup<H: Holder<Word = AtomicU32>>(call: &Call) {
    match call.arity(0, 1).and_then(|()| call.long(1, 1)) {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream casts zend_long to int"
        )]
        Ok(count) => {
            let woken = word::<H>(call).map(|word| futex_wakeup(word, count as i32));
            call.set_bool(woken.is_some_and(|woken| woken >= 0));
        }
        Err(_) => failed(call),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_wrap_and_truncate_like_upstream() {
        let word = AtomicU32::new(u32::MAX);
        assert_eq!(word.add(2), 1, "32-bit add wraps");
        word.set(-1);
        assert_eq!(
            word.get(),
            i64::from(u32::MAX),
            "store truncates the PHP int"
        );
        assert!(word.cmpset(-1, 7), "CAS compares the truncated value");
        assert_eq!(word.get(), 7, "and stores the new one");
        let long = AtomicI64::new(i64::MAX);
        assert_eq!(long.add(1), i64::MIN, "64-bit add wraps");
    }
}
