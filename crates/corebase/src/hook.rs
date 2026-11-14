// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors).
//! The global hook table behind `swoole_add_hook`, `swoole_call_hook` and
//! `swoole_isset_hook`: per hook type, its callbacks in call order.
use crate::constants::SW_MAX_HOOK_TYPE;
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// `swHookFunc`.
pub type HookFn = unsafe extern "C" fn(*mut c_void);

/// A hook type: `swGlobalHookType`, or PHP's `php_swoole_hook_type` above
/// `SW_GLOBAL_HOOK_USER`. C names one by any int, which an unknown type ignores.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookType(c_int);

impl HookType {
    pub const BEFORE_SERVER_START: Self = Self(0);
    pub const BEFORE_CLIENT_START: Self = Self(1);
    pub const BEFORE_WORKER_START: Self = Self(2);
    pub const ON_CORO_START: Self = Self(3);
    pub const ON_CORO_STOP: Self = Self(4);
    pub const ON_REACTOR_CREATE: Self = Self(5);
    pub const BEFORE_SERVER_SHUTDOWN: Self = Self(6);
    pub const AFTER_SERVER_SHUTDOWN: Self = Self(7);
    pub const BEFORE_WORKER_STOP: Self = Self(8);
    pub const ON_REACTOR_DESTROY: Self = Self(9);
    pub const BEFORE_SERVER_CREATE: Self = Self(10);
    pub const AFTER_SERVER_CREATE: Self = Self(11);
    pub const AFTER_FORK: Self = Self(12);
    /// `PHP_SWOOLE_HOOK_BEFORE_ENABLE_HOOK` (`SW_GLOBAL_HOOK_USER`).
    pub const BEFORE_ENABLE_HOOK: Self = Self(24);
    pub const AFTER_ENABLE_HOOK: Self = Self(25);
    pub const BEFORE_REQUEST: Self = Self(26);
    pub const AFTER_RESPONSE: Self = Self(27);
}

type List = &'static [HookFn];

/// Every list any hook type has had, oldest first: each change writes a new
/// snapshot and leaks the old one, so readers need no lock and no allocation,
/// and a child after `fork` can't block on a lock another thread held (#3576).
/// Hooks are registered at module init, a few at most.
static SNAPSHOTS: [OnceLock<List>; 256] = [const { OnceLock::new() }; 256];

/// Per hook type, its list's index in `SNAPSHOTS` plus one; 0 for none.
static LISTS: [AtomicUsize; SW_MAX_HOOK_TYPE as usize] =
    [const { AtomicUsize::new(0) }; SW_MAX_HOOK_TYPE as usize];

/// Snapshots written so far, under which writers serialise.
static WRITTEN: Mutex<usize> = Mutex::new(0);

/// Hook type `kind`'s current snapshot index, or None for an unknown type.
fn slot(kind: HookType) -> Option<&'static AtomicUsize> {
    LISTS.get(usize::try_from(kind.0).ok()?)
}

/// Hook type `kind`'s list: empty for none or an unknown type.
fn list(kind: HookType) -> List {
    slot(kind)
        .and_then(|slot| slot.load(Ordering::Acquire).checked_sub(1))
        .and_then(|index| SNAPSHOTS.get(index)?.get().copied())
        .unwrap_or(&[])
}

/// `void swoole_add_hook(swGlobalHookType, swHookFunc, int push_back)`.
#[cfg_attr(feature = "upstream-symbols", unsafe(no_mangle))]
pub extern "C" fn swoole_add_hook(kind: HookType, func: HookFn, push_back: c_int) {
    let Some(slot) = slot(kind) else {
        return;
    };
    let mut written = WRITTEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut next = list(kind).to_vec();
    if push_back != 0 {
        next.push(func);
    } else {
        next.insert(0, func);
    }
    let Some(snapshot) = SNAPSHOTS.get(*written) else {
        crate::fatal("more than 256 hook registrations");
    };
    snapshot.get_or_init(|| Box::leak(next.into_boxed_slice()));
    *written += 1;
    slot.store(*written, Ordering::Release);
}

/// `void swoole_call_hook(swGlobalHookType, void *arg)`: each callback in turn,
/// over the list as it was when the call began.
///
/// # Safety
/// The callbacks accept `arg`.
#[cfg_attr(feature = "upstream-symbols", unsafe(no_mangle))]
pub unsafe extern "C" fn swoole_call_hook(kind: HookType, arg: *mut c_void) {
    for func in list(kind) {
        // SAFETY: the caller's contract.
        unsafe { func(arg) };
    }
}

/// `swoole_call_hook(SW_GLOBAL_HOOK_AFTER_FORK, nullptr)`, for whoever forked: the
/// one hook type whose callbacks are passed nothing.
pub fn call_after_fork() {
    // SAFETY: an after-fork hook is registered to be called with no argument, which
    // is what it gets.
    unsafe { swoole_call_hook(HookType::AFTER_FORK, std::ptr::null_mut()) };
}

/// `bool swoole_isset_hook(swGlobalHookType)`.
#[cfg_attr(feature = "upstream-symbols", unsafe(no_mangle))]
#[must_use]
pub extern "C" fn swoole_isset_hook(kind: HookType) -> bool {
    !list(kind).is_empty()
}

/// `swoole_clean`'s release of every hook list.
pub fn clear() {
    let _written = WRITTEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for slot in &LISTS {
        slot.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn set_9999(data: *mut c_void) {
        // SAFETY: the test passes an i32.
        unsafe { *data.cast::<i32>() = 9999 };
    }

    unsafe extern "C" fn double(data: *mut c_void) {
        // SAFETY: the test passes an i32.
        unsafe { *data.cast::<i32>() *= 2 };
    }

    // core-tests base.hook, over SW_GLOBAL_HOOK_END (31), then the call order
    // push_back selects. One test owns the table.
    #[test]
    fn hook() {
        const END: HookType = HookType(31);
        let mut count = 0_i32;
        swoole_add_hook(END, set_9999, 1);
        assert!(swoole_isset_hook(END));
        // SAFETY: the callback takes an i32.
        unsafe { swoole_call_hook(END, (&raw mut count).cast()) };
        assert_eq!(count, 9999);

        swoole_add_hook(END, double, 0);
        count = 1;
        // SAFETY: as above.
        unsafe { swoole_call_hook(END, (&raw mut count).cast()) };
        assert_eq!(count, 9999, "the front callback runs first");

        clear();
        assert!(!swoole_isset_hook(END));
        let outside = HookType(SW_MAX_HOOK_TYPE.cast_signed());
        assert!(!swoole_isset_hook(HookType(-1)) && !swoole_isset_hook(outside));
    }
}
