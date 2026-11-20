//! `proc_terminate`, `proc_close` and `proc_get_status` for the coroutine `proc_open`
//! (`HOOK_PROC`). The process record, its resource type and destructor stay in
//! thirdparty `proc_open.cc`, read here through its accessors.
use core::{
    ffi::{c_int, c_void},
    marker::PhantomData,
    ptr::NonNull,
};
use swoole_rs_corebase::{CCast, Switch, built_with};
use swoole_rs_zend::{Call, Value, ZStr};

/// `proc_open.cc`'s `sw_php_process_handle` behind the resource argument `'a` borrows.
/// Not `Copy`: closing takes it, after which the record is freed.
#[repr(transparent)]
struct Process<'a>(NonNull<c_void>, PhantomData<&'a Value>);

unsafe extern "C" {
    /// The record behind `zproc`'s resource, or none once Zend has thrown.
    safe fn swr_proc_fetch(zproc: &Value) -> Option<Process<'_>>;
    safe fn swr_proc_child(proc: &Process<'_>) -> libc::pid_t;
    safe fn swr_proc_command<'a>(proc: &'a Process<'_>) -> &'a ZStr;
    safe fn swr_proc_set_running(proc: &Process<'_>, running: bool);
    /// Closes `zproc`'s resource, freeing the record; the wait status its destructor
    /// reaped.
    safe fn swr_proc_close(proc: Process<'_>, zproc: &Value) -> c_int;
}

swoole_rs_zend::handlers! {
    zif_swoole_proc_terminate => terminate;
    zif_swoole_proc_close => close;
    zif_swoole_proc_get_status => get_status;
}

/// `_co_waitpid`: `System::waitpid_safe` in a Swoole-thread build, else `System::waitpid`.
fn waitpid(pid: libc::pid_t, status: &mut c_int, options: c_int) -> libc::pid_t {
    if built_with(Switch::Thread) {
        swoole_rs_hooks::exec::system_waitpid_safe(pid, Some(status), options)
    } else {
        swoole_rs_corebase::reactor::wait_upstream::system_waitpid(pid, status, options, -1.0)
    }
}

/// `proc_terminate(resource $process, int $signal = SIGTERM): bool`.
fn terminate(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.resource(1)?, call.long(2, libc::SIGTERM.into())?)));
    let Ok((zproc, signal)) = parsed else {
        return;
    };
    let Some(proc) = swr_proc_fetch(zproc) else {
        return;
    };
    // `kill(pid, sig_no)`: the int conversion keeps the low 32 bits.
    let killed = swoole_rs_corebase::sys::kill(swr_proc_child(&proc), signal.c_cast::<c_int>());
    call.set_bool(killed.is_ok());
}

/// `proc_close(resource $process): int`.
fn close(call: &Call) {
    let Ok(zproc) = call.arity(1, 1).and_then(|()| call.resource(1)) else {
        return;
    };
    let Some(proc) = swr_proc_fetch(zproc) else {
        return;
    };
    call.set_long(swr_proc_close(proc, zproc).into());
}

/// `proc_get_status(resource $process): array`.
fn get_status(call: &Call) {
    let Ok(zproc) = call.arity(1, 1).and_then(|()| call.resource(1)) else {
        return;
    };
    let Some(proc) = swr_proc_fetch(zproc) else {
        return;
    };
    let child = swr_proc_child(&proc);
    call.set_array();
    call.add_assoc_stringl(c"command", swr_proc_command(&proc).bytes());
    call.add_assoc_long(c"pid", child.into());
    let (mut running, mut signaled, mut stopped) = (true, false, false);
    let (mut exitcode, mut termsig, mut stopsig) = (-1, 0, 0);
    let mut wstatus = 0;
    swoole_rs_corebase::sys::set_errno(0);
    let waited = waitpid(child, &mut wstatus, libc::WNOHANG | libc::WUNTRACED);
    if waited == child {
        if libc::WIFEXITED(wstatus) {
            running = false;
            exitcode = libc::WEXITSTATUS(wstatus);
        }
        if libc::WIFSIGNALED(wstatus) {
            running = false;
            signaled = true;
            termsig = libc::WTERMSIG(wstatus);
        }
        if libc::WIFSTOPPED(wstatus) {
            stopped = true;
            stopsig = libc::WSTOPSIG(wstatus);
        }
    } else if waited == -1 {
        // ECHILD: the pid is gone or not this process's child.
        running = false;
    }
    swr_proc_set_running(&proc, running);
    call.add_assoc_bool(c"running", running);
    call.add_assoc_bool(c"signaled", signaled);
    call.add_assoc_bool(c"stopped", stopped);
    call.add_assoc_long(c"exitcode", exitcode.into());
    call.add_assoc_long(c"termsig", termsig.into());
    call.add_assoc_long(c"stopsig", stopsig.into());
}
