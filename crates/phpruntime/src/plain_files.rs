//! The coroutine plain-files wrapper's path operations: `php_plain_files_url_stater`,
//! `php_plain_files_unlink`, `php_plain_files_rename`, `php_plain_files_mkdir`,
//! `php_plain_files_rmdir` and `php_plain_files_metadata` of upstream's copy of PHP's
//! plain-files wrapper (`thirdparty/php/streams/plain_wrapper.c`), whose wrapper
//! table names them. A warning runs the user error handler, so each traps a bailout
//! and raises it last.
use crate::stream::raise;
use libc::{c_char, c_int, c_long, gid_t, mode_t, uid_t};
use std::ffi::{CStr, CString, c_void};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::sys::{Text, errno, strerror};
use swoole_rs_hooks::hooked::swoole_coroutine_close;
use swoole_rs_hooks::stdio::{
    swoole_coroutine_access, swoole_coroutine_mkdir, swoole_coroutine_open,
    swoole_coroutine_rename, swoole_coroutine_rmdir, swoole_coroutine_unlink,
};
use swoole_rs_zend::{
    E_WARNING, ZTS,
    bailout::{self, Bailout},
    report,
};

unsafe extern "C" {
    /// `stat(2)` and `lstat(2)`, which upstream's wrapper leaves blocking, under the
    /// names of the 64-bit-inode and 64-bit-time versions where libc has two.
    #[cfg_attr(
        all(target_os = "macos", target_arch = "x86_64"),
        link_name = "stat$INODE64"
    )]
    #[cfg_attr(target_os = "netbsd", link_name = "__stat50")]
    safe fn stat(path: Text<'_>, status: &mut libc::stat) -> c_int;
    #[cfg_attr(
        all(target_os = "macos", target_arch = "x86_64"),
        link_name = "lstat$INODE64"
    )]
    #[cfg_attr(target_os = "netbsd", link_name = "__lstat50")]
    safe fn lstat(path: Text<'_>, status: &mut libc::stat) -> c_int;
    /// `chown(2)`, `chmod(2)` and `umask(2)`.
    safe fn chown(path: Text<'_>, owner: uid_t, group: gid_t) -> c_int;
    safe fn chmod(path: Text<'_>, mode: mode_t) -> c_int;
    safe fn umask(mask: mode_t) -> mode_t;
    /// `utime(2)`: the times given, or now for none.
    safe fn utime(path: Text<'_>, times: Option<&libc::utimbuf>) -> c_int;
    /// `php_get_uid_by_name` and `php_get_gid_by_name`: the id of the user or group
    /// `name` in the slot given, `SUCCESS` (0) when there is one. They ask libc's
    /// user database and run no PHP code.
    safe fn php_get_uid_by_name(name: Text<'_>, uid: &mut uid_t) -> c_int;
    safe fn php_get_gid_by_name(name: Text<'_>, gid: &mut gid_t) -> c_int;
    /// `expand_filepath_with_mode(path, real_path, NULL, 0, use_realpath)`: the
    /// absolute form of `path` in `real_path`, a `MAXPATHLEN` buffer, or null. It
    /// reads this thread's working directory and runs no PHP code.
    safe fn expand_filepath_with_mode(
        path: Text<'_>,
        real_path: &mut [u8; MAXPATHLEN],
        relative_to: Option<Text<'_>>,
        relative_to_len: usize,
        use_realpath: c_int,
    ) -> *mut c_char;
    /// `php_error_docref2(NULL, param1, param2, level, "%s", message)` inside
    /// `zend_try`. 1 if it bailed out.
    safe fn swr_zend_try_error2(
        param1: Text<'_>,
        param2: Text<'_>,
        level: c_int,
        message: Text<'_>,
    ) -> c_int;
    /// `php_copy_file(source, target)` into `copied`, inside `zend_try`: it opens
    /// both through their stream wrappers, which may warn. 1 if it bailed out.
    safe fn swr_zend_try_copy_file(source: Text<'_>, target: Text<'_>, copied: &mut bool) -> c_int;
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
/// `PHP_STREAM_MKDIR_RECURSIVE`, `CWD_EXPAND` and `MAXPATHLEN`.
const MKDIR_RECURSIVE: c_int = 1;
const CWD_EXPAND: c_int = 0;
const MAXPATHLEN: usize = libc::PATH_MAX.unsigned_abs() as usize;
/// `PHP_STREAM_META_TOUCH`, `_OWNER_NAME`, `_OWNER`, `_GROUP_NAME`, `_GROUP` and `_ACCESS`.
const META_TOUCH: c_int = 1;
const META_OWNER_NAME: c_int = 2;
const META_OWNER: c_int = 3;
const META_GROUP_NAME: c_int = 4;
const META_GROUP: c_int = 5;
const META_ACCESS: c_int = 6;

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
    warn_text(path, &[strerror(code).as_bytes()])
}

/// `php_error_docref1(NULL, path, E_WARNING, "%s", message)`, the message being
/// `parts` end to end: a path or a name goes in as its bytes, as C's `%s` prints it.
fn warn_text(path: &CStr, parts: &[&[u8]]) -> Result<(), Bailout> {
    let message = CString::new(parts.concat()).unwrap_or_default();
    bailout::trapped(swr_zend_try_error1(
        path.into(),
        E_WARNING,
        message.as_c_str().into(),
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

/// `php_error_docref2(NULL, from, to, E_WARNING, "%s", strerror(code))`.
fn warn2(from: &CStr, to: &CStr, code: c_int) -> Result<(), Bailout> {
    let reason = CString::new(strerror(code)).unwrap_or_default();
    bailout::trapped(swr_zend_try_error2(
        from.into(),
        to.into(),
        E_WARNING,
        reason.as_c_str().into(),
    ))
}

/// `php_error_docref(NULL, E_WARNING, "%s", strerror(code))`.
fn warn_code(code: c_int) -> Result<(), Bailout> {
    report(E_WARNING, &CString::new(strerror(code)).unwrap_or_default())
}

/// `php_plain_files_rename`: 1 once `to` names what `from` did, else 0.
fn rename(from: Option<&CStr>, to: Option<&CStr>) -> Result<c_int, Bailout> {
    let (Some(from), Some(to)) = (from, to) else {
        return Ok(0);
    };
    let (from, to) = (local(from), local(to));
    if refused(from, true)? || refused(to, true)? {
        return Ok(0);
    }
    if swoole_coroutine_rename(from.into(), to.into()) == -1 {
        let code = errno();
        if code == libc::EXDEV {
            // The umask is the process's: upstream leaves it alone under ZTS.
            let mask = (!ZTS).then(|| umask(0o077));
            let moved = copy_across(from, to);
            if let Some(mask) = mask {
                umask(mask);
            }
            return moved;
        }
        warn2(from, to, code)?;
        return Ok(0);
    }
    php_clear_stat_cache(true, None, 0);
    Ok(1)
}

/// A rename across filesystems: copy, give the copy the source's owner and mode
/// (which may fail with `EPERM` for anyone but root), then unlink the source.
fn copy_across(from: &CStr, to: &CStr) -> Result<c_int, Bailout> {
    let mut copied = false;
    bailout::trapped(swr_zend_try_copy_file(from.into(), to.into(), &mut copied))?;
    let Some(source) = copied
        .then(|| swoole_rs_corebase::sys::stat(from))
        .flatten()
    else {
        warn2(from, to, errno())?;
        return Ok(0);
    };
    // The group first, so the environment's umask covers the file meanwhile.
    let mut moved = true;
    if chown(to.into(), source.st_uid, source.st_gid) != 0 {
        let code = errno();
        warn2(from, to, code)?;
        moved = code == libc::EPERM;
    }
    if moved && chmod(to.into(), source.st_mode) != 0 {
        let code = errno();
        warn2(from, to, code)?;
        moved = code == libc::EPERM;
    }
    if moved {
        swoole_coroutine_unlink(from.into());
    }
    Ok(moved.into())
}

/// The C string `bytes` holds: up to its first NUL.
fn text(bytes: &[u8]) -> CString {
    let end = bytes.iter().position(|&byte| byte == 0);
    CString::new(bytes.get(..end.unwrap_or(bytes.len())).unwrap_or_default()).unwrap_or_default()
}

/// `strrchr(path + from, '/')`: the last slash of the C string that starts at `from`.
fn last_slash(path: &[u8], from: usize) -> Option<usize> {
    let rest = path.get(from..)?;
    let end = rest
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(rest.len());
    let at = rest.get(..end)?.iter().rposition(|&byte| byte == b'/')?;
    Some(from + at)
}

/// The byte at `at`, or the NUL that ends the path.
fn byte(path: &[u8], at: usize) -> u8 {
    path.get(at).copied().unwrap_or(0)
}

/// Writes `value` at `at`, which is inside the path.
fn put(path: &mut [u8], at: usize, value: u8) {
    if let Some(slot) = path.get_mut(at) {
        *slot = value;
    }
}

/// `php_plain_files_mkdir`: 1 once the directory exists, else 0.
fn mkdir(dir: &CStr, mode: c_int, options: c_int) -> Result<c_int, Bailout> {
    let dir = local(dir);
    let mode = mode.c_cast::<mode_t>();
    if options & MKDIR_RECURSIVE == 0 {
        // `php_mkdir`: it warns whatever the options.
        if refused(dir, true)? {
            return Ok(0);
        }
        if swoole_coroutine_mkdir(dir.into(), mode) < 0 {
            warn_code(errno())?;
            return Ok(0);
        }
        return Ok(1);
    }
    let mut expanded = [0; MAXPATHLEN];
    if expand_filepath_with_mode(dir.into(), &mut expanded, None, 0, CWD_EXPAND).is_null() {
        report(E_WARNING, c"Invalid path")?;
        return Ok(0);
    }
    // The path's bytes; a NUL written into them below ends the prefix being tried.
    let mut path = text(&expanded).into_bytes();
    if refused(&text(&path), true)? {
        return Ok(0);
    }
    let (dir_len, end) = (dir.to_bytes().len(), path.len());
    // Upstream looks for the directory separator from the end, to save work.
    let mut at = path.iter().take(dir_len).position(|&byte| byte == b'/');
    let offset = at.map_or(0, |slash| slash + 1);
    if !(at.is_some() && dir_len == 1) {
        // Find the top-level directory to create: cut the path at each slash from
        // the end until what is left exists.
        loop {
            at = last_slash(&path, offset)
                .or_else(|| (offset != 1).then(|| last_slash(&path, 0)).flatten());
            let Some(mut slash) = at else { break };
            let mut extra = 0;
            put(&mut path, slash, 0);
            while slash > 0 && byte(&path, slash - 1) == b'/' {
                extra += 1;
                slash -= 1;
                put(&mut path, slash, 0);
            }
            if swoole_rs_corebase::sys::stat(&text(&path)).is_some() {
                loop {
                    put(&mut path, slash, b'/');
                    if extra == 0 {
                        break;
                    }
                    extra -= 1;
                    slash += 1;
                }
                at = Some(slash);
                break;
            }
        }
    }
    let mut at = at.unwrap_or(0);
    loop {
        let made = swoole_coroutine_mkdir(text(&path).as_c_str().into(), mode) >= 0;
        let code = errno();
        if !made && code != libc::EEXIST {
            if options & REPORT_ERRORS != 0 {
                warn_code(code)?;
            }
            return Ok(0);
        }
        // Put back the next slash that was cut, and with it the next directory.
        let mut restored = false;
        loop {
            at += 1;
            if at >= end {
                break;
            }
            if byte(&path, at) == 0 {
                restored = true;
                put(&mut path, at, b'/');
                if byte(&path, at + 1) != 0 {
                    break;
                }
            }
        }
        if at >= end || !restored {
            // No directory is left; the last one existing already is the failure.
            if !made {
                if options & REPORT_ERRORS != 0 {
                    warn_code(code)?;
                }
                return Ok(0);
            }
            return Ok(1);
        }
    }
}

/// `php_plain_files_metadata` (`touch`, `chown`, `chgrp`, `chmod`): 1 once done, else
/// 0. Its C entry reads `value` as the option types it: `times` for a touch, `name`
/// for an owner or group by name, `number` for an id or a mode.
fn metadata(
    url: &CStr,
    option: c_int,
    times: Option<&libc::utimbuf>,
    name: Option<&CStr>,
    number: c_long,
) -> Result<c_int, Bailout> {
    let path = local(url);
    if refused(path, true)? {
        return Ok(0);
    }
    let name = name.unwrap_or_default();
    let status = match option {
        META_TOUCH => {
            if swoole_coroutine_access(path.into(), libc::F_OK) != 0 {
                let flags = libc::O_CREAT | libc::O_WRONLY | libc::O_TRUNC;
                let file = swoole_coroutine_open(path.into(), flags, 0o666);
                if file == -1 {
                    let reason = strerror(errno());
                    warn_text(
                        path,
                        &[
                            b"Unable to create file ",
                            path.to_bytes(),
                            b" because ",
                            reason.as_bytes(),
                        ],
                    )?;
                    return Ok(0);
                }
                swoole_coroutine_close(file);
            }
            utime(path.into(), times)
        }
        META_OWNER_NAME | META_OWNER => {
            let mut uid = number.c_cast::<uid_t>();
            if option == META_OWNER_NAME && php_get_uid_by_name(name.into(), &mut uid) != 0 {
                warn_text(path, &[b"Unable to find uid for ", name.to_bytes()])?;
                return Ok(0);
            }
            // `(gid_t) -1`: the group stays.
            chown(path.into(), uid, gid_t::MAX)
        }
        META_GROUP_NAME | META_GROUP => {
            let mut gid = number.c_cast::<gid_t>();
            if option == META_GROUP_NAME && php_get_gid_by_name(name.into(), &mut gid) != 0 {
                warn_text(path, &[b"Unable to find gid for ", name.to_bytes()])?;
                return Ok(0);
            }
            chown(path.into(), uid_t::MAX, gid)
        }
        META_ACCESS => chmod(path.into(), number.c_cast::<mode_t>()),
        _ => {
            let message = format!("Unknown option {option} for stream_metadata");
            swoole_rs_zend::value_error(&CString::new(message).unwrap_or_default());
            return Ok(0);
        }
    };
    if status == -1 {
        warn_text(path, &[b"Operation failed: ", strerror(errno()).as_bytes()])?;
        return Ok(0);
    }
    php_clear_stat_cache(false, None, 0);
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

/// The wrapper's `rename`.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phpruntime_plain_rename(
    _wrapper: *mut c_void,
    from: Option<Text<'_>>,
    to: Option<Text<'_>>,
    _options: c_int,
    _context: *mut c_void,
) -> c_int {
    raise(rename(from.map(Text::get), to.map(Text::get)), 0)
}

/// The wrapper's `mkdir`.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phpruntime_plain_mkdir(
    _wrapper: *mut c_void,
    dir: Text<'_>,
    mode: c_int,
    options: c_int,
    _context: *mut c_void,
) -> c_int {
    raise(mkdir(dir.get(), mode, options), 0)
}

/// The wrapper's `metadata`, with `value` as its C entry read it for `option`.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phpruntime_plain_metadata(
    url: Text<'_>,
    option: c_int,
    times: Option<&libc::utimbuf>,
    name: Option<Text<'_>>,
    number: c_long,
) -> c_int {
    raise(
        metadata(url.get(), option, times, name.map(Text::get), number),
        0,
    )
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

    #[test]
    fn a_cut_path_reads_as_c_does() {
        use super::{last_slash, text};
        let mut path = b"/a/b//c".to_vec();
        assert_eq!(last_slash(&path, 1), Some(5));
        path[5] = 0;
        path[4] = 0;
        assert_eq!(text(&path).as_bytes(), b"/a/b");
        assert_eq!(last_slash(&path, 1), Some(2));
        // From past the cut, the string is what follows it.
        assert_eq!(last_slash(&path, 6), None);
        assert_eq!(last_slash(b"/a", 1), None);
        assert_eq!(last_slash(b"/a", 0), Some(0));
        assert_eq!(last_slash(b"", 1), None);
    }
}
