//! `swoole_module_entry`, which `get_module()` hands Zend (through crates/ffi's
//! `phpswoole_module`, with the families above this crate), with its
//! dependencies, and the tail of `SwooleG.fatal_error`.
use crate::{constants, info, module, optional, table};
use std::{
    ffi::{CStr, c_int},
    io::Write,
    ptr,
    sync::{
        OnceLock,
        atomic::{AtomicPtr, Ordering},
    },
};
use swoole_rs_corebase::{Switch as C, built_with};
use swoole_rs_zend::{
    Fatal, bailout,
    sys::{zend_module_dep, zend_module_entry},
};

unsafe extern "C" {
    /// `*entry = { STANDARD_MODULE_HEADER_EX, ..., STANDARD_MODULE_PROPERTIES }`:
    /// it overwrites the record with the build's module header.
    safe fn swr_zend_module_template(entry: &mut zend_module_entry);
}

/// `MODULE_DEP_REQUIRED` and `MODULE_DEP_CONFLICTS`.
const REQUIRED: u8 = 1;
const CONFLICTS: u8 = 2;
/// `SW_ERROR_CO_HAS_BEEN_BOUND`.
const CO_HAS_BEEN_BOUND: c_int = swoole_rs_error::ErrorCode::SW_ERROR_CO_HAS_BEEN_BOUND as c_int;

/// Zend retains the entry and invokes its callbacks without a Rust context.
/// The extension links the feature-specific family crates, which depend on this
/// crate for their lifecycle services. Publish their hooks with the entry Zend uses.
struct Module {
    entry: AtomicPtr<zend_module_entry>,
    families: &'static module::Families,
}

static MODULE: OnceLock<Module> = OnceLock::new();

/// The family hooks, available from the first completed `get_module()` onward.
#[must_use]
pub fn families() -> Option<&'static module::Families> {
    MODULE.get().map(|module| module.families)
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
    MODULE
        .get_or_init(|| Module {
            entry: AtomicPtr::new(build_entry()),
            families,
        })
        .entry
        .load(Ordering::Relaxed)
}

fn build_entry() -> *mut zend_module_entry {
    let mut entry: zend_module_entry = swoole_rs_zend::zeroed();
    swr_zend_module_template(&mut entry);
    entry.deps = Box::leak(deps().into_boxed_slice()).as_ptr();
    entry.name = c"swoole".as_ptr();
    entry.functions = table::functions();
    swoole_rs_zend::Lifecycle {
        startup: crate::swr_v1_phpswoole_minit,
        shutdown: module::swr_v1_phpswoole_mshutdown,
        request_startup: module::swr_v1_phpswoole_rinit,
        request_shutdown: module::swr_v1_phpswoole_rshutdown,
        info: info::swr_v1_phpswoole_minfo,
    }
    .install(&mut entry);
    entry.version = constants::VERSION.as_ptr();
    Box::into_raw(Box::new(entry))
}

swoole_rs_zend::exports! {
/// `fatal_error`'s tail, after `swr_zend_fatal_error` threw and printed the
/// `Swoole\Error`: a coroutine bound to a socket prints its backtrace, then
/// the thread bails out (outside the main thread) or the process exits 255.
/// Only that shim calls it: only it makes the [`Fatal`] it lends.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpswoole_fatal_exit(error: Fatal<'_>) {
    if error.is(CO_HAS_BEEN_BOUND) {
        let cid = swoole_rs_corebase::coroutine::socket_bound_cid();
        let rule = "-------------------------------------------------------------------";
        // Upstream's fprintf: nothing to do when stderr fails.
        write!(
            std::io::stderr(),
            "\n [Coroutine-{cid}] Stack trace:\n {rule}\n"
        )
        .unwrap_or_default();
        // The coroutine's backtrace, printed; a bailout is trapped.
        if let Some(families) = families()
            && let Err(bailed) = (families.print_backtrace)(error.php(), cid)
        {
            bailout::defer(Err(bailed));
            return;
        }
    }
    // Outside the main thread, the thread's bailout; this frame owns nothing.
    if !optional::main_thread() {
        swoole_rs_zend::thread::defer_exit();
        return;
    }
    swoole_rs_corebase::swoole_exit(255);
}
}
