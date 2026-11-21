//! `swoole_module_entry`, which `get_module()` hands Zend (through crates/ffi's
//! `phpswoole_module`, with the families above this crate), with its
//! dependencies, and the tail of `SwooleG.fatal_error`.
use crate::{constants, info, module, optional, table};
use std::{
    ffi::{CStr, c_int},
    io::Write,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_corebase::{Switch as C, built_with};
use swoole_rs_zend::{
    bailout,
    sys::{swr_zend_module_template, zend_module_dep, zend_module_entry},
};

/// `MODULE_DEP_REQUIRED` and `MODULE_DEP_CONFLICTS`.
const REQUIRED: u8 = 1;
const CONFLICTS: u8 = 2;
/// `SW_ERROR_CO_HAS_BEEN_BOUND`.
const CO_HAS_BEEN_BOUND: c_int = 10002;

static ENTRY: AtomicPtr<zend_module_entry> = AtomicPtr::new(ptr::null_mut());

fn dep(name: &'static CStr, kind: u8) -> zend_module_dep {
    zend_module_dep {
        name: name.as_ptr(),
        rel: ptr::null(),
        version: ptr::null(),
        type_: kind,
    }
}

/// `swoole_deps[]`: json always, the rest as the build's `#ifdef`s decide.
fn deps() -> Vec<zend_module_dep> {
    let mut deps = vec![dep(c"json", REQUIRED)];
    for (capability, name) in [
        (C::Mysqlnd, c"mysqlnd"),
        (C::Sockets, c"sockets"),
        (C::Curl, c"curl"),
    ] {
        if built_with(capability) {
            deps.push(dep(name, REQUIRED));
        }
    }
    if [C::Pgsql, C::Odbc, C::Oracle, C::Sqlite, C::Firebird]
        .into_iter()
        .any(built_with)
    {
        deps.push(dep(c"pdo", REQUIRED));
    }
    for (capability, name) in [(C::Ftp, c"ftp"), (C::Ssh2, c"ssh2")] {
        if built_with(capability) {
            deps.push(dep(name, CONFLICTS));
        }
    }
    // ZEND_MOD_END.
    deps.push(zend_module_dep {
        name: ptr::null(),
        rel: ptr::null(),
        version: ptr::null(),
        type_: 0,
    });
    deps
}

/// `get_module()`: the module entry, built once, whose phases run `families`.
pub fn module_entry(families: &'static module::Families) -> *mut zend_module_entry {
    module::register(families);
    let current = ENTRY.load(Ordering::Acquire);
    if !current.is_null() {
        return current;
    }
    let mut entry: zend_module_entry = swoole_rs_zend::zeroed();
    // SAFETY: a live record.
    unsafe { swr_zend_module_template(&raw mut entry) };
    entry.deps = Box::leak(deps().into_boxed_slice()).as_ptr();
    entry.name = c"swoole".as_ptr();
    entry.functions = table::functions();
    entry.module_startup_func = Some(crate::swr_v1_phpswoole_minit);
    entry.module_shutdown_func = Some(module::swr_v1_phpswoole_mshutdown);
    entry.request_startup_func = Some(module::swr_v1_phpswoole_rinit);
    entry.request_shutdown_func = Some(module::swr_v1_phpswoole_rshutdown);
    entry.info_func = Some(info::swr_v1_phpswoole_minfo);
    entry.version = constants::VERSION.as_ptr();
    let entry = Box::into_raw(Box::new(entry));
    ENTRY.store(entry, Ordering::Release);
    entry
}

/// `fatal_error`'s tail, after `swr_zend_fatal_error` threw and printed the
/// `Swoole\Error`: a coroutine bound to a socket prints its backtrace, then
/// the thread bails out (outside the main thread) or the process exits 255.
/// # Safety
/// From `swr_zend_fatal_error`, after MINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpswoole_fatal_exit(code: c_int) {
    if code == CO_HAS_BEEN_BOUND {
        let scheduler = swoole_rs_coroutine::scheduler();
        // SAFETY: this thread's scheduler, live for the thread.
        let cid = unsafe { (*scheduler).socket_bound_cid };
        let rule = "-------------------------------------------------------------------";
        // Upstream's fprintf: nothing to do when stderr fails.
        write!(
            std::io::stderr(),
            "\n [Coroutine-{cid}] Stack trace:\n {rule}\n"
        )
        .ok();
        // The coroutine's backtrace, printed; a bailout is trapped.
        if let Some(families) = module::families()
            && let Err(bailed) = (families.print_backtrace)(cid)
        {
            bailout::raise(Err(bailed));
            return;
        }
    }
    // Outside the main thread, the thread's bailout; this frame owns nothing.
    optional::thread_bailout();
    swoole_rs_corebase::swoole_exit(255);
}
