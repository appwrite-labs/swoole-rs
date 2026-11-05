//! Event-loop admission, lifetime and shutdown ordering.
use super::{has_reactor, ops};
use std::{
    ffi::{c_char, c_void},
    ptr,
};
use swoole_rs_corebase::constants::EventInitFlag;
use swoole_rs_zend::{
    bailout::{self, Bailout, trap},
    sys as z,
};

/// The native reactor/SDK leaves; this table is static, registered once.
#[repr(C)]
pub(crate) struct Ops {
    cli: unsafe extern "C" fn() -> u8,
    shutdown_begun: unsafe extern "C" fn() -> u8,
    server: unsafe extern "C" fn() -> u8,
    fatal: unsafe extern "C" fn() -> u8,
    coroutine: unsafe extern "C" fn() -> u8,
    bailed: unsafe extern "C" fn() -> u8,
    create: unsafe extern "C" fn(i32) -> i32,
    installed: unsafe extern "C" fn() -> u8,
    install: unsafe extern "C" fn(*mut c_void),
    trace: unsafe extern "C" fn(*mut c_void),
    shutdown: unsafe extern "C" fn(*mut c_void),
    user_condition: unsafe extern "C" fn() -> u8,
    condition_installed: unsafe extern "C" fn() -> u8,
    condition_set: unsafe extern "C" fn(*mut c_void),
    if_exit: unsafe extern "C" fn(*mut c_void),
    wait: unsafe extern "C" fn(*mut c_void),
    free: unsafe extern "C" fn(*mut c_void),
    timers: unsafe extern "C" fn(*mut c_void),
    running: unsafe extern "C" fn(u8),
    once: unsafe extern "C" fn(u8),
    report: unsafe extern "C" fn(i32, *const c_char),
    system_error: unsafe extern "C" fn(i32, i32, *const c_char),
}

fn native() -> &'static Ops {
    &ops().driver
}

fn report(level: u32, message: &std::ffi::CStr) -> Result<(), Bailout> {
    // SAFETY: the static native reporter runs inside the Zend trap.
    unsafe { bailout::report_level(native().report, level.cast_signed(), message) }
}

fn effect(call: unsafe extern "C" fn(*mut c_void)) -> Result<(), Bailout> {
    // SAFETY: this leaf takes no data and any PHP reentry is contained in C.
    unsafe { trap(call, ptr::null_mut()) }
}

fn wait_once() -> Result<(), Bailout> {
    let mut status = 0_i32;
    // SAFETY: the wait leaf writes its result; PHP callbacks remain inside the C trap.
    unsafe { trap(native().wait, (&raw mut status).cast()) }?;
    if status < 0 {
        let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        // SAFETY: the C reporter runs inside Zend's C trap with the captured errno.
        unsafe {
            bailout::report_code(
                native().system_error,
                code,
                z::E_ERROR.cast_signed(),
                c"reactor wait failed",
            )
        }?;
    }
    Ok(())
}

pub(super) fn system_warning(message: &std::ffi::CStr, code: i32) -> Result<(), Bailout> {
    // SAFETY: the C reporter runs inside Zend's C trap with the captured errno.
    unsafe {
        bailout::report_code(
            native().system_error,
            code,
            z::E_WARNING.cast_signed(),
            message,
        )
    }
}

pub(super) fn init() -> Result<i32, Bailout> {
    // SAFETY: immutable request/module facts, queried only after MINIT.
    if unsafe { (native().cli)() } == 0 {
        report(z::E_ERROR, c"async-io must be used in PHP CLI mode")?;
        return Ok(-1);
    }
    // SAFETY: bit0 presence, bit1 task worker, bit2 coroutine enabled, bit3 manager.
    let server = unsafe { (native().server)() };
    if server & 1 != 0 {
        if server & 2 != 0 && server & 4 == 0 {
            report(z::E_ERROR, c"Unable to use async-io in task processes, please set `task_enable_coroutine` to true")?;
            return Ok(-1);
        }
        if server & 8 != 0 {
            report(z::E_ERROR, c"Unable to use async-io in manager process")?;
            return Ok(-1);
        }
    }
    if !has_reactor() {
        effect(native().trace)?;
        // SAFETY: a fresh reactor, with the canonical wait-on-exit flag.
        if unsafe { (native().create)(EventInitFlag::SW_EVENTLOOP_WAIT_EXIT as i32) } < 0 {
            report(z::E_ERROR, c"Unable to create event-loop reactor")?;
            return Ok(-1);
        }
        // SAFETY: the native SDK registration borrows the static callback name.
        unsafe {
            trap(
                native().shutdown,
                c"swoole_event_rshutdown".as_ptr().cast_mut().cast(),
            )
        }?;
    }
    // SAFETY: these inspect the live global reactor and immutable callable presence.
    let user_condition = unsafe { (native().user_condition)() } != 0;
    // SAFETY: the condition registry of a live reactor is read without executing it.
    let installed = has_reactor() && unsafe { (native().condition_installed)() } != 0;
    if has_reactor() && user_condition && !installed {
        effect(native().condition_set)?;
    }
    Ok(0)
}

pub(super) fn check() -> Result<(), Bailout> {
    ensure()?;
    // SAFETY: admission installed a reactor; only the handler presence is read.
    if unsafe { (native().installed)() } == 0 {
        effect(native().install)?;
    }
    Ok(())
}

pub(super) fn ensure() -> Result<i32, Bailout> {
    // SAFETY: request-state inspection has no PHP effects.
    if unsafe { (native().shutdown_begun)() } != 0 {
        return Ok(-1);
    }
    if has_reactor() {
        return Ok(0);
    }
    Ok(if init()? == 0 { 1 } else { -1 })
}

pub(super) fn wait() -> Result<(), Bailout> {
    // SAFETY: a request-local error-state getter.
    if unsafe { (native().fatal)() } != 0 || !has_reactor() {
        return Ok(());
    }
    // SAFETY: the current coroutine identity is queried without running PHP.
    if unsafe { (native().coroutine)() } != 0 {
        report(z::E_ERROR, c"Unable to call Event::wait() in coroutine")?;
        return Ok(());
    }
    let mut exit = 0_u8;
    // SAFETY: exit conditions can run PHP, so the native query executes inside C's trap.
    unsafe { trap(native().if_exit, (&raw mut exit).cast()) }?;
    // SAFETY: the reactor's bailout flag is only read after its exit check.
    if exit == 0 && unsafe { (native().bailed)() } == 0 {
        // SAFETY: this SDK-only leaf edits executor flags without running PHP.
        let shutdown = unsafe { z::swr_zend_event_pause_shutdown() };
        wait_once()?;
        // SAFETY: restore the exact prior shutdown bit after a normally completed wait.
        unsafe { z::swr_zend_event_restore_shutdown(shutdown) };
    }
    effect(native().free)
}

pub(super) fn exit() -> Result<(), Bailout> {
    if has_reactor() {
        effect(native().timers)?;
        // SAFETY: the native setter updates the current reactor's primitive running flag.
        unsafe { (native().running)(0) };
    }
    Ok(())
}

pub(super) fn dispatch() -> Result<bool, Bailout> {
    if !has_reactor() {
        return Ok(false);
    }
    // SAFETY: a live reactor's primitive once flag; no PHP callback runs in the setter.
    unsafe { (native().once)(1) };
    wait_once()?;
    // SAFETY: restore after the native wait returned normally, matching the original ordering.
    unsafe { (native().once)(0) };
    Ok(true)
}

pub(super) fn shutdown() -> Result<(), Bailout> {
    // SAFETY: request-local error-state getter; do not enter the loop after a fatal error.
    if unsafe { (native().fatal)() } == 0 && has_reactor() {
        // SAFETY: only the live reactor bailout flag is inspected.
        if unsafe { (native().bailed)() } == 0 {
            report(
                z::E_DEPRECATED,
                c"Event::wait() in shutdown function is deprecated",
            )?;
        }
        wait()?;
    }
    Ok(())
}
