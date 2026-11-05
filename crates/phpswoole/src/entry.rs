//! `swoole_module_entry`, which `get_module()` hands Zend, with its
//! dependencies, and the tail of `SwooleG.fatal_error`.
use crate::{
    HostOps, OPS, constants,
    info::{self, Info},
    info_inventory::Capability as C,
    module, ops, table,
};
use std::{
    ffi::{CStr, c_int, c_long},
    io::Write,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::{
    bailout::{self, Bailout},
    sys::{swr_zend_module_template, zend_module_dep, zend_module_entry, zval},
};

/// `MODULE_DEP_REQUIRED` and `MODULE_DEP_CONFLICTS`.
const REQUIRED: u8 = 1;
const CONFLICTS: u8 = 2;
/// `SW_ERROR_CO_HAS_BEEN_BOUND`.
const CO_HAS_BEEN_BOUND: c_int = 10002;

static ENTRY: AtomicPtr<zend_module_entry> = AtomicPtr::new(ptr::null_mut());

unsafe extern "C" {
    fn swr_v1_phpcoroutines_print_backtrace(
        cid: c_long,
        options: c_long,
        limit: c_long,
        return_value: *mut zval,
    ) -> u8;
}

fn dep(name: &'static CStr, kind: u8) -> zend_module_dep {
    zend_module_dep {
        name: name.as_ptr(),
        rel: ptr::null(),
        version: ptr::null(),
        type_: kind,
    }
}

/// `swoole_deps[]`: json always, the rest as the build's `#ifdef`s decide.
fn deps(info: &Info) -> Vec<zend_module_dep> {
    let mut deps = vec![dep(c"json", REQUIRED)];
    for (capability, name) in [
        (C::Mysqlnd, c"mysqlnd"),
        (C::Sockets, c"sockets"),
        (C::Curl, c"curl"),
    ] {
        if info.has(capability) {
            deps.push(dep(name, REQUIRED));
        }
    }
    if [C::Pgsql, C::Odbc, C::Oracle, C::Sqlite, C::Firebird]
        .into_iter()
        .any(|c| info.has(c))
    {
        deps.push(dep(c"pdo", REQUIRED));
    }
    for (capability, name) in [(C::Ftp, c"ftp"), (C::Ssh2, c"ssh2")] {
        if info.has(capability) {
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

/// `get_module()`: the module entry, built once, after which every Rust
/// function may use `ops`.
/// # Safety
/// `ops` is C++'s static table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpswoole_module(ops: *const HostOps) -> *mut zend_module_entry {
    let current = ENTRY.load(Ordering::Acquire);
    if !current.is_null() {
        return current;
    }
    // SAFETY: per the caller.
    let Some(ops) = (unsafe { ops.as_ref() }) else {
        return ptr::null_mut();
    };
    OPS.get_or_init(|| ops);
    // SAFETY: a plain C record of pointers, integers and bytes, for which zero is valid.
    let mut info: Info = unsafe { std::mem::zeroed() };
    // SAFETY: C++ fills it with its compiler and library facts.
    unsafe { (ops.info)(&raw mut info) };
    // SAFETY: a plain C record, for which zero is valid; the template fills its header.
    let mut entry: zend_module_entry = unsafe { std::mem::zeroed() };
    // SAFETY: a live record.
    unsafe { swr_zend_module_template(&raw mut entry) };
    entry.deps = Box::leak(deps(&info).into_boxed_slice()).as_ptr();
    entry.name = c"swoole".as_ptr();
    // SAFETY: C++'s static handler arrays, or null.
    entry.functions = unsafe { table::functions(ops.stdext, ops.ssh2, ops.ftp) };
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
    let ops = ops();
    if code == CO_HAS_BEEN_BOUND {
        // SAFETY: C++'s scheduler.
        let cid = unsafe { (ops.socket_bound_cid)() };
        let rule = "-------------------------------------------------------------------";
        // Upstream's fprintf: nothing to do when stderr fails.
        write!(
            std::io::stderr(),
            "\n [Coroutine-{cid}] Stack trace:\n {rule}\n"
        )
        .ok();
        // SAFETY: the coroutine's backtrace, printed; a bailout is trapped.
        if unsafe { swr_v1_phpcoroutines_print_backtrace(cid as c_long, 0, 0, ptr::null_mut()) }
            != 0
        {
            bailout::raise(Err(Bailout));
            return;
        }
    }
    // SAFETY: C++'s thread test.
    if unsafe { (ops.main_thread)() } == 0 {
        // SAFETY: the thread's bailout; this frame owns nothing.
        unsafe { (ops.thread_bailout)() };
    }
    // SAFETY: Swoole's exit.
    unsafe { (ops.exit)(255) };
}
