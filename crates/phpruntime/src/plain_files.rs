//! The coroutine plain-files wrapper's path operations: `php_plain_files_url_stater`,
//! `php_plain_files_unlink` and `php_plain_files_rmdir` of upstream's copy of PHP's
//! plain-files wrapper (`thirdparty/php/streams/plain_wrapper.c`), whose wrapper
//! table names them. A warning runs the user error handler, so each traps a bailout
//! and raises it last.
use crate::stream::raise;
use libc::c_int;
use std::ffi::{CStr, CString, c_void};
use swoole_rs_corebase::sys::{Text, errno, strerror};
use swoole_rs_hooks::stdio::{swoole_coroutine_rmdir, swoole_coroutine_unlink};
use swoole_rs_zend::{
    E_WARNING,
    bailout::{self, Bailout},
};

unsafe extern "C" {
    /// `stat(2)` and `lstat(2)`, which upstream's wrapper leaves blocking.
    safe fn stat(path: Text<'_>, status: &mut libc::stat) -> c_int;
    safe fn lstat(path: Text<'_>, status: &mut libc::stat) -> c_int;
    /// `php_clear_stat_cache`: this PHP thread's stat cache, and with
    /// `clear_realpath_cache` its realpath cache. It runs no PHP code.
    safe fn php_clear_stat_cache(
        clear_realpath_cache: bool,
        filename: Option<Text<'_>>,
        filename_len: usize,
    );
    /// `php_check_open_basedir_ex(path, warn)` into `refused`, inside `zend_try`:
    /// with `warn`, a refusal is an `E_WARNING`. 1 if it bailed out.
    safe fn swr_zend_try_open_basedir(path: Text<'_>, warn: c_int, refused: &mut bool) -> c_int;
    /// `php_error_docref1(NULL, param, level, "%s", message)` inside `zend_try`. 1 if
    /// it bailed out.
    safe fn swr_zend_try_error1(param: Text<'_>, level: c_int, message: Text<'_>) -> c_int;
}

/// `PHP_STREAM_URL_STAT_LINK`, `_QUIET` and `_IGNORE_OPEN_BASEDIR`, and `REPORT_ERRORS`.
const STAT_LINK: c_int = 1;
const STAT_QUIET: c_int = 2;
const STAT_IGNORE_OPEN_BASEDIR: c_int = 4;
const REPORT_ERRORS: c_int = 8;

/// `url` past a leading `file://`, which upstream finds with `strncasecmp`.
fn local(url: &CStr) -> &CStr {
    const SCHEME: &[u8] = b"file://";
    url.to_bytes_with_nul()
        .split_at_checked(SCHEME.len())
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case(SCHEME))
        .and_then(|(_, path)| CStr::from_bytes_with_nul(path).ok())
        .unwrap_or(url)
}

/// `php_check_open_basedir_ex(path, warn)`: whether `open_basedir` refuses the path.
fn refused(path: &CStr, warn: bool) -> Result<bool, Bailout> {
    let mut refused = true;
    bailout::trapped(swr_zend_try_open_basedir(
        path.into(),
        warn.into(),
        &mut refused,
    ))?;
    Ok(refused)
}

/// `php_error_docref1(NULL, path, E_WARNING, "%s", strerror(code))`.
fn warn(path: &CStr, code: c_int) -> Result<(), Bailout> {
    let reason = CString::new(strerror(code)).unwrap_or_default();
    bailout::trapped(swr_zend_try_error1(
        path.into(),
        E_WARNING,
        reason.as_c_str().into(),
    ))
}

/// `php_plain_files_url_stater`: 0 with the path's status, or -1.
fn url_stat(url: &CStr, flags: c_int, status: &mut libc::stat) -> Result<c_int, Bailout> {
    let mut path = url;
    if flags & STAT_IGNORE_OPEN_BASEDIR == 0 {
        path = local(url);
        if refused(path, flags & STAT_QUIET == 0)? {
            return Ok(-1);
        }
    }
    Ok(if flags & STAT_LINK == 0 {
        stat(path.into(), status)
    } else {
        lstat(path.into(), status)
    })
}

/// `php_plain_files_unlink`: 1 once the file is gone, else 0.
fn unlink(url: &CStr, options: c_int) -> Result<c_int, Bailout> {
    let path = local(url);
    if refused(path, true)? {
        return Ok(0);
    }
    if swoole_coroutine_unlink(path.into()) == -1 {
        let code = errno();
        if options & REPORT_ERRORS != 0 {
            warn(path, code)?;
        }
        return Ok(0);
    }
    // The stat cache and the realpath cache.
    php_clear_stat_cache(true, None, 0);
    Ok(1)
}

/// `php_plain_files_rmdir`: 1 once the directory is gone, else 0 with a warning.
fn rmdir(url: &CStr) -> Result<c_int, Bailout> {
    let path = local(url);
    if refused(path, true)? {
        return Ok(0);
    }
    if swoole_coroutine_rmdir(path.into()) < 0 {
        warn(path, errno())?;
        return Ok(0);
    }
    php_clear_stat_cache(true, None, 0);
    Ok(1)
}

swoole_rs_zend::exports! {
/// The wrapper's `url_stat`: `ssb` is a `php_stream_statbuf`, whose one member is the
/// `zend_stat_t` (`struct stat`) written here.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phpruntime_plain_url_stat(
    _wrapper: *mut c_void,
    url: Text<'_>,
    flags: c_int,
    ssb: &mut libc::stat,
    _context: *mut c_void,
) -> c_int {
    raise(url_stat(url.get(), flags, ssb), -1)
}

/// The wrapper's `unlink`.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phpruntime_plain_unlink(
    _wrapper: *mut c_void,
    url: Text<'_>,
    options: c_int,
    _context: *mut c_void,
) -> c_int {
    raise(unlink(url.get(), options), 0)
}

/// The wrapper's `rmdir`.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phpruntime_plain_rmdir(
    _wrapper: *mut c_void,
    url: Text<'_>,
    _options: c_int,
    _context: *mut c_void,
) -> c_int {
    raise(rmdir(url.get()), 0)
}
}

#[cfg(test)]
mod tests {
    use super::local;

    #[test]
    fn a_file_scheme_is_dropped_whatever_its_case() {
        assert_eq!(local(c"file:///tmp/a"), c"/tmp/a");
        assert_eq!(local(c"FILE:///tmp/a"), c"/tmp/a");
        assert_eq!(local(c"file://"), c"");
        assert_eq!(local(c"/tmp/file://a"), c"/tmp/file://a");
        assert_eq!(local(c"file:/"), c"file:/");
    }
}
