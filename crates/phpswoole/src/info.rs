//! PHP information output and conditional row ordering owned by the module.
use std::ffi::{CStr, CString, c_char, c_void};
use swoole_rs_corebase::{Switch as C, built_with, switches::SWITCHES};
use swoole_rs_zend::{
    bailout::{self, Bailout},
    sys::{
        swr_zend_try_info_end, swr_zend_try_info_header, swr_zend_try_info_ini,
        swr_zend_try_info_row, swr_zend_try_info_start, zend_module_entry,
    },
};

/// The headers' and libraries' versions (native/phphelpers/host.c), with no
/// PHP output or row policy.
#[repr(C)]
pub(crate) struct Info {
    built: *const c_char,
    openssl: *const c_char,
    curl: *const c_char,
    cares: *const c_char,
    zlib: *const c_char,
    zstd: *const c_char,
}
unsafe extern "C" {
    fn swr_phpswoole_versions(info: *mut Info);
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
    fn enabled(first: C, last: C) -> Result<(), Bailout> {
        for &(switch, _, label) in SWITCHES {
            if (first as u8..=last as u8).contains(&(switch as u8)) && built_with(switch) {
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
        row(c"Version", crate::constants::VERSION.as_ptr())?;
        let abi = swoole_rs_error::swr_v1_abi_version();
        let features = swoole_rs_error::swr_v1_feature_bits();
        text(
            c"Rust migration bridge",
            &format!("ABI {abi}, features {features}"),
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
            if built_with(C::ThreadContext) {
                c"enabled with thread context"
            } else if built_with(C::AsmContext) {
                c"enabled with boost asm context"
            } else {
                c"enabled with ucontext"
            }
            .as_ptr(),
        )?;
        Self::enabled(C::Debug, C::Sockets)?;
        row(c"openssl", self.openssl)?;
        Self::enabled(C::Dtls, C::Dtls)?;
        row(c"http2", c"enabled".as_ptr())?;
        row(c"json", c"enabled".as_ptr())?;
        if built_with(C::Curl) {
            Self::enabled(C::Curl, C::Curl)?;
            row(c"curl-version", self.curl)?;
        }
        if built_with(C::Cares) {
            row(c"c-ares", self.cares)?;
        }
        if built_with(C::Zlib) {
            row(c"zlib", self.zlib)?;
        }
        if built_with(C::Brotli) {
            text(c"brotli", &swoole_rs_brotli::versions())?;
        }
        if built_with(C::Zstd) {
            row(c"zstd", self.zstd)?;
        }
        Self::enabled(C::Timedlock, C::Tcmalloc)?;
        #[cfg(feature = "pgsql")]
        {
            text(c"postgresql(libpq) version", &swoole_rs_pgsql::version())?;
            row(c"coroutine_pgsql", c"enabled".as_ptr())?;
        }
        Self::enabled(C::Odbc, C::UringSocket)?;
        if built_with(C::Stacktrace) {
            Self::enabled(C::Stacktrace, C::Stacktrace)?;
        } else {
            Self::enabled(C::Execinfo, C::Execinfo)?;
        }
        // SAFETY: ssh2's and ftp's MINFO, a native body whose bailout the trap's
        // setjmp catches below every Rust frame; PHP lends its live module entry.
        unsafe { bailout::trap(crate::optional::client_minfo, module) }?;
        // SAFETY: closes the PHP information table; output bailouts are contained.
        contained(unsafe { swr_zend_try_info_end() })?;
        // SAFETY: the live module entry selects its registered INI entries; guarded below Rust.
        contained(unsafe { swr_zend_try_info_ini(module) })
    }
}
/// `PHP_MINFO_FUNCTION(swoole)`: the module's complete PHP information table
/// and INI entries, with the headers' and libraries' versions.
/// # Safety
/// Zend's MINFO, with the module's live entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpswoole_minfo(module: *mut zend_module_entry) {
    let null = std::ptr::null();
    let mut info = Info {
        built: null,
        openssl: null,
        curl: null,
        cares: null,
        zlib: null,
        zstd: null,
    };
    // SAFETY: host.c fills it with static strings.
    unsafe { swr_phpswoole_versions(&raw mut info) };
    bailout::raise(info.output(module.cast()));
}
