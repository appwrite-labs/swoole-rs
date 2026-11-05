//! PHP information output and conditional row ordering owned by the module.
use crate::info_inventory::{CAPABILITIES, Capability as C};
use std::ffi::{CStr, CString, c_char, c_void};
use swoole_rs_zend::{
    bailout::{self, Bailout},
    sys::{
        swr_zend_try_info_end, swr_zend_try_info_header, swr_zend_try_info_ini,
        swr_zend_try_info_row, swr_zend_try_info_start, zend_module_entry,
    },
};

/// Native compiler and dependency facts, with no PHP output or row policy.
#[repr(C)]
pub(crate) struct Info {
    flags: u64,
    abi: u32,
    features: u64,
    version: *const c_char,
    built: *const c_char,
    openssl: *const c_char,
    curl: *const c_char,
    cares: *const c_char,
    zlib: *const c_char,
    zstd: *const c_char,
    brotli_encoder: u32,
    brotli_decoder: u32,
    libpq: [c_char; 16],
}
unsafe extern "C" {
    fn swr_cpp_phpswoole_info_ssh2() -> u8;
    fn swr_cpp_phpswoole_info_ftp(module: *mut c_void) -> u8;
}
fn contained(bailed: impl Into<i32>) -> Result<(), Bailout> {
    if bailed.into() == 0 {
        Ok(())
    } else {
        Err(Bailout)
    }
}
fn row(key: &CStr, value: *const c_char) -> Result<(), Bailout> {
    // SAFETY: keys are static and values are live NUL-terminated native or Rust strings;
    // this leaf traps output-handler bailouts below every Rust string owner.
    contained(unsafe { swr_zend_try_info_row(key.as_ptr(), value) })
}
fn text(key: &CStr, value: &str) -> Result<(), Bailout> {
    let value = CString::new(value).map_err(|_| Bailout)?;
    row(key, value.as_ptr())
}
impl Info {
    pub(crate) fn has(&self, capability: C) -> bool {
        self.flags & (1 << capability as u8) != 0
    }
    fn enabled(&self, first: C, last: C) -> Result<(), Bailout> {
        for &(capability, _, label) in CAPABILITIES {
            if (first as u8..=last as u8).contains(&(capability as u8)) && self.has(capability) {
                row(label, c"enabled".as_ptr())?;
            }
        }
        Ok(())
    }
    fn output(&self, module: *mut c_void) -> Result<(), Bailout> {
        // SAFETY: a single PHP output effect is guarded below this Rust frame.
        contained(unsafe { swr_zend_try_info_start() })?;
        // SAFETY: both header strings have static lifetime; output bailouts are trapped.
        contained(unsafe { swr_zend_try_info_header(c"Swoole".as_ptr(), c"enabled".as_ptr()) })?;
        row(c"Author", c"Swoole Team <team@swoole.com>".as_ptr())?;
        row(c"Version", self.version)?;
        text(
            c"Rust migration bridge",
            &format!("ABI {}, features {}", self.abi, self.features),
        )?;
        row(c"Built", self.built)?;
        row(
            c"host byte order",
            if cfg!(target_endian = "little") {
                c"little endian"
            } else {
                c"big endian"
            }
            .as_ptr(),
        )?;
        row(
            c"coroutine",
            if self.has(C::ThreadContext) {
                c"enabled with thread context"
            } else if self.has(C::AsmContext) {
                c"enabled with boost asm context"
            } else {
                c"enabled with ucontext"
            }
            .as_ptr(),
        )?;
        self.enabled(C::Debug, C::Sockets)?;
        row(c"openssl", self.openssl)?;
        self.enabled(C::Dtls, C::Dtls)?;
        row(c"http2", c"enabled".as_ptr())?;
        row(c"json", c"enabled".as_ptr())?;
        if self.has(C::Curl) {
            self.enabled(C::Curl, C::Curl)?;
            row(c"curl-version", self.curl)?;
        }
        if self.has(C::Cares) {
            row(c"c-ares", self.cares)?;
        }
        if self.has(C::Zlib) {
            row(c"zlib", self.zlib)?;
        }
        if self.has(C::Brotli) {
            text(
                c"brotli",
                &format!("E{}/D{}", self.brotli_encoder, self.brotli_decoder),
            )?;
        }
        if self.has(C::Zstd) {
            row(c"zstd", self.zstd)?;
        }
        self.enabled(C::Timedlock, C::Tcmalloc)?;
        if self.has(C::Pgsql) {
            row(c"postgresql(libpq) version", self.libpq.as_ptr())?;
            row(c"coroutine_pgsql", c"enabled".as_ptr())?;
        }
        self.enabled(C::Odbc, C::UringSocket)?;
        if self.has(C::Stacktrace) {
            self.enabled(C::Stacktrace, C::Stacktrace)?;
        } else {
            self.enabled(C::Execinfo, C::Execinfo)?;
        }
        if self.has(C::Ssh2) {
            // SAFETY: optional extension output is a single native hook with its own trap.
            contained(unsafe { swr_cpp_phpswoole_info_ssh2() })?;
        }
        if self.has(C::Ftp) {
            // SAFETY: PHP lends its live module entry to this guarded optional MINFO hook.
            contained(unsafe { swr_cpp_phpswoole_info_ftp(module) })?;
        }
        // SAFETY: closes the PHP information table; output bailouts are contained.
        contained(unsafe { swr_zend_try_info_end() })?;
        // SAFETY: the live module entry selects its registered INI entries; guarded below Rust.
        contained(unsafe { swr_zend_try_info_ini(module) })
    }
}
/// `PHP_MINFO_FUNCTION(swoole)`: the module's complete PHP information table
/// and INI entries, from C++'s compiler and library facts.
/// # Safety
/// Zend's MINFO, with the module's live entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpswoole_minfo(module: *mut zend_module_entry) {
    // SAFETY: a plain C record of pointers, integers and bytes, for which zero is valid.
    let mut info: Info = unsafe { std::mem::zeroed() };
    // SAFETY: C++ fills it with static strings and its libpq version.
    unsafe { (crate::ops().info)(&raw mut info) };
    bailout::raise(info.output(module.cast()));
}
