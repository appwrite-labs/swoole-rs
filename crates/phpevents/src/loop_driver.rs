//! Event-loop admission, lifetime and shutdown ordering.
use crate::native::{self, has_reactor};
use swoole_rs_zend::{E_ERROR, bailout::Bailout, sys as z};

fn wait_once() -> Result<(), Bailout> {
    if native::wait()? < 0 {
        let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        native::system_error(code, E_ERROR, c"reactor wait failed")?;
    }
    Ok(())
}

pub(super) fn init() -> Result<i32, Bailout> {
    if !native::cli() {
        native::fatal(E_ERROR, c"async-io must be used in PHP CLI mode")?;
        return Ok(-1);
    }
    if let Some(refusal) = native::server_refusal() {
        native::fatal(E_ERROR, refusal)?;
        return Ok(-1);
    }
    if !has_reactor() && !native::create() {
        native::fatal(E_ERROR, c"Unable to create event-loop reactor")?;
        return Ok(-1);
    }
    swoole_rs_phpcoroutines::install_exit_condition();
    Ok(0)
}

pub(super) fn check(php: swoole_rs_zend::Php<'_>) -> Result<(), Bailout> {
    ensure()?;
    super::install_handlers(php);
    Ok(())
}

pub(super) fn ensure() -> Result<i32, Bailout> {
    if native::shutdown_begun() {
        return Ok(-1);
    }
    if has_reactor() {
        return Ok(0);
    }
    Ok(if init()? == 0 { 1 } else { -1 })
}

pub(super) fn wait() -> Result<(), Bailout> {
    if swoole_rs_zend::last_fatal_error() || !has_reactor() {
        return Ok(());
    }
    if swoole_rs_phpcoroutines::in_coroutine() {
        native::fatal(E_ERROR, c"Unable to call Event::wait() in coroutine")?;
        return Ok(());
    }
    if !native::if_exit()? && !native::bailed() {
        let shutdown = swoole_rs_zend::pause_shutdown();
        wait_once()?;
        // The exact prior shutdown bit, after a normally completed wait.
        swoole_rs_zend::restore_shutdown(shutdown);
    }
    super::release_cycle_tasks();
    let freed = native::free();
    freed.and(swoole_rs_zend::bailout::outcome(()))
}

pub(super) fn exit() -> Result<(), Bailout> {
    if has_reactor() {
        native::clear_timers()?;
        swoole_rs_reactor::event_api::swoole_event_set_running(false);
    }
    Ok(())
}

pub(super) fn dispatch() -> Result<bool, Bailout> {
    if !has_reactor() {
        return Ok(false);
    }
    swoole_rs_reactor::event_api::swoole_event_set_once(true);
    wait_once()?;
    swoole_rs_reactor::event_api::swoole_event_set_once(false);
    Ok(true)
}

pub(super) fn shutdown() -> Result<(), Bailout> {
    // Do not enter the loop after a fatal error.
    if !swoole_rs_zend::last_fatal_error() && has_reactor() {
        if !native::bailed() {
            native::fatal(
                z::E_DEPRECATED.cast_signed(),
                c"Event::wait() in shutdown function is deprecated",
            )?;
        }
        wait()?;
    }
    Ok(())
}
