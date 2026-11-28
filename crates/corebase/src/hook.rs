// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors).
//! The global hook table behind `swoole_add_hook`, `swoole_call_hook` and
//! `swoole_isset_hook`: per hook type, its callbacks in call order.
use crate::constants::SW_MAX_HOOK_TYPE;
use std::ffi::{c_int, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// `swHookFunc`.
pub type HookFn = unsafe extern "C" fn(*mut c_void);

static HOOKS: Mutex<[Vec<HookFn>; SW_MAX_HOOK_TYPE as usize]> =
    Mutex::new([const { Vec::new() }; SW_MAX_HOOK_TYPE as usize]);

/// One bit per hook type whose list is non-empty, so `swoole_isset_hook` (on every
/// coroutine start and stop, and every HTTP request) is a load, as upstream's
/// pointer test is, rather than a lock. Written only under the table's lock.
static SET: AtomicU32 = AtomicU32::new(0);

fn table() -> std::sync::MutexGuard<'static, [Vec<HookFn>; SW_MAX_HOOK_TYPE as usize]> {
    HOOKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Run `f` on hook type `kind`'s list, or return `None` for an unknown type.
fn with<R>(kind: c_int, f: impl FnOnce(&mut Vec<HookFn>) -> R) -> Option<R> {
    let kind = usize::try_from(kind).ok()?;
    table().get_mut(kind).map(f)
}

/// `void swoole_add_hook(swGlobalHookType, swHookFunc, int push_back)`.
#[cfg_attr(feature = "upstream-symbols", unsafe(no_mangle))]
pub extern "C" fn swoole_add_hook(kind: c_int, func: HookFn, push_back: c_int) {
    with(kind, |list| {
        if push_back != 0 {
            list.push(func);
        } else {
            list.insert(0, func);
        }
        SET.fetch_or(1_u32 << kind, Ordering::Release);
    });
}

/// `void swoole_call_hook(swGlobalHookType, void *arg)`: each callback in turn,
/// over the list as it was when the call began.
///
/// # Safety
/// The callbacks accept `arg`.
#[cfg_attr(feature = "upstream-symbols", unsafe(no_mangle))]
pub unsafe extern "C" fn swoole_call_hook(kind: c_int, arg: *mut c_void) {
    let list = with(kind, |list| list.clone()).unwrap_or_default();
    for func in list {
        // SAFETY: the caller's contract.
        unsafe { func(arg) };
    }
}

/// `bool swoole_isset_hook(swGlobalHookType)`.
#[cfg_attr(feature = "upstream-symbols", unsafe(no_mangle))]
#[must_use]
pub extern "C" fn swoole_isset_hook(kind: c_int) -> bool {
    u32::try_from(kind)
        .ok()
        .and_then(|kind| SET.load(Ordering::Acquire).checked_shr(kind))
        .is_some_and(|set| set & 1 != 0)
}

/// `swoole_clean`'s release of every hook list.
pub fn clear() {
    let mut table = table();
    table.iter_mut().for_each(Vec::clear);
    SET.store(0, Ordering::Release);
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
        const END: c_int = 31;
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
        assert!(!swoole_isset_hook(-1) && !swoole_isset_hook(SW_MAX_HOOK_TYPE.cast_signed()));
    }
}
