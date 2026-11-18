//! PHP information output and conditional row ordering owned by the module.
use std::ffi::{CStr, CString, c_int};
use swoole_rs_corebase::{Switch as C, built_with, switches::SWITCHES, sys::Text};
use swoole_rs_zend::{
    Minfo,
    bailout::{self, Bailout},
    contain_with,
};

/// The headers' and libraries' versions (native/phphelpers/host.c), with no
/// PHP output or row policy: static strings, or none.
// `repr(C)`: C's `swr_phpswoole_versions` (`native/phphelpers/host.c`), which fills it.
#[repr(C)]
#[derive(Default)]
pub(crate) struct Info {
    built: Option<Text<'static>>,
    openssl: Option<Text<'static>>,
    curl: Option<Text<'static>>,
    cares: Option<Text<'static>>,
    zlib: Option<Text<'static>>,
    zstd: Option<Text<'static>>,
}
unsafe extern "C" {
    /// Fills every field with a static string, or leaves it null.
    safe fn swr_phpswoole_versions(info: &mut Info);
}

// crates/zend-sys' information-table shims: each prints the strings it is lent,
// which it only reads, inside `zend_try`, and returns whether output bailed out.
unsafe extern "C" {
    safe fn swr_zend_try_info_start() -> c_int;
    safe fn swr_zend_try_info_header(key: Text<'_>, value: Text<'_>) -> c_int;
    /// A null value prints as none, as `php_info_print_table_row` does.
    safe fn swr_zend_try_info_row(key: Text<'_>, value: Option<Text<'_>>) -> c_int;
    safe fn swr_zend_try_info_end() -> c_int;
}

fn contained(bailed: impl Into<i32>) -> Result<(), Bailout> {
    if bailed.into() == 0 {
        Ok(())
    } else {
        Err(Bailout)
    }
}
fn row<'a>(key: &CStr, value: impl Into<Option<Text<'a>>>) -> Result<(), Bailout> {
    contained(swr_zend_try_info_row(key.into(), value.into()))
}
fn text(key: &CStr, value: &str) -> Result<(), Bailout> {
    let value = CString::new(value).map_err(|_| Bailout)?;
    row(key, Text::from(value.as_c_str()))
}
impl Info {
    fn enabled(first: C, last: C) -> Result<(), Bailout> {
        for &(switch, _, label) in SWITCHES {
            if (first as u8..=last as u8).contains(&(switch as u8)) && built_with(switch) {
                row(label, Text::from(c"enabled"))?;
            }
        }
        Ok(())
    }
    fn output(&self, module: Minfo<'_>) -> Result<(), Bailout> {
        contained(swr_zend_try_info_start())?;
        contained(swr_zend_try_info_header(
            c"Swoole".into(),
            c"enabled".into(),
        ))?;
        row(c"Author", Text::from(c"Swoole Team <team@swoole.com>"))?;
        row(c"Version", Text::from(crate::constants::VERSION))?;
        // The workspace version and the commit tools/build_ext.py builds from.
        text(
            c"swoole-rs",
            &format!(
                "{} ({})",
                env!("CARGO_PKG_VERSION"),
                option_env!("SWOOLE_RS_COMMIT").unwrap_or("unknown")
            ),
        )?;
        let abi = swoole_rs_error::swr_v1_abi_version();
        let features = swoole_rs_error::swr_v1_feature_bits();
        text(
            c"Rust migration bridge",
            &format!("ABI {abi}, features {features}"),
        )?;
        row(c"Built", self.built)?;
        let order = if cfg!(target_endian = "little") {
            c"little endian"
        } else {
            c"big endian"
        };
        row(c"host byte order", Text::from(order))?;
        let coroutine = if built_with(C::ThreadContext) {
            c"enabled with thread context"
        } else if built_with(C::AsmContext) {
            c"enabled with boost asm context"
        } else {
            c"enabled with ucontext"
        };
        row(c"coroutine", Text::from(coroutine))?;
        Self::enabled(C::Debug, C::Sockets)?;
        row(c"openssl", self.openssl)?;
        Self::enabled(C::Dtls, C::Dtls)?;
        row(c"http2", Text::from(c"enabled"))?;
        row(c"json", Text::from(c"enabled"))?;
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
            row(c"coroutine_pgsql", Text::from(c"enabled"))?;
        }
        Self::enabled(C::Odbc, C::UringSocket)?;
        if built_with(C::Stacktrace) {
            Self::enabled(C::Stacktrace, C::Stacktrace)?;
        } else {
            Self::enabled(C::Execinfo, C::Execinfo)?;
        }
        // ssh2's MINFO, whose bailout the trap catches below every Rust frame.
        contain_with(crate::optional::client_minfo)?;
        contained(swr_zend_try_info_end())?;
        module.ini_entries()
    }
}
swoole_rs_zend::exports! {
/// `PHP_MINFO_FUNCTION(swoole)`: the module's complete PHP information table
/// and INI entries, with the headers' and libraries' versions.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpswoole_minfo(module: Minfo<'_>) {
    let mut info = Info::default();
    swr_phpswoole_versions(&mut info);
    bailout::defer(info.output(module));
}

}
