//! `curl_exec`, libcurl's callbacks for a `CurlHandle` and the options that set them (#5132): the
//! callable options of `curl_setopt`, `CURLINFO_HEADER_OUT`, and the check for streams
//! closed under a handle.
//!
//! What a handle keeps stays in the PHP `php_curl`, whose layout is ext/curl's own (PHP's
//! unhooked functions read the same objects), behind the accessors in
//! `native/hooks/curl_handle.cc`. PHP code runs in every callable, and can set an option of
//! the handle it runs for, copy it or yield the coroutine, so nothing read from the handle
//! is kept across a call into PHP, and each call is contained.
use super::php_handle::{PhpHandle, argument, checked, swoole_curl_easy};
use super::setopt::{StdioFile, libcurl, swoole_curl_file_stream, swoole_curl_save_error, warn};
use crate::Easy;
use std::ffi::{CStr, CString, c_int, c_void};
use std::marker::PhantomData;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::sys::Text;
use swoole_rs_zend::sys::{IS_UNDEF, PHP_VERSION_ID, zend_long};
use swoole_rs_zend::{Call, E_WARNING, Owned, Value, ZStr, ZString, Zvals, bailout, contain_with};

/// `PHP_CURL_*`: what a write, header or read callback does with its data.
const STDOUT: c_int = 0;
const FILE: c_int = 1;
const USER: c_int = 2;
const DIRECT: c_int = 3;
const RETURN: c_int = 4;
const IGNORE: c_int = 7;

/// The file-handle options, which name a handle's streams.
const CURLOPT_FILE: c_int = 10001;
const CURLOPT_INFILE: c_int = 10009;
const CURLOPT_WRITEHEADER: c_int = 10029;
const CURLOPT_STDERR: c_int = 10037;

/// `CURL_READFUNC_ABORT`.
const READ_ABORT: usize = 0x1000_0000;
/// `CURLINFO_HEADER_OUT`, as libcurl's debug callback names the request header.
const HEADER_OUT: c_int = 2;

/// The bytes libcurl lends a callback, beside their length.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Data<'a>(*const c_void, PhantomData<&'a [u8]>);

/// The buffer libcurl's read callback fills, beside its size.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Fill<'a>(*mut c_void, PhantomData<&'a mut [u8]>);

unsafe extern "C" {
    safe fn swoole_curl_copy_streams(copy: PhpHandle<'_>, source: PhpHandle<'_>);
    safe fn swoole_curl_copy_callback(
        copy: PhpHandle<'_>,
        source: PhpHandle<'_>,
        option: c_int,
        data_option: c_int,
    );

    safe fn swoole_curl_stream_gone(value: &Owned) -> bool;
    // The stream's option goes back to its default: no file, and the method it started with.
    safe fn swoole_curl_stream_reset(handle: PhpHandle<'_>, option: c_int);
    safe fn swoole_curl_free_streams(handle: PhpHandle<'_>);
    safe fn swoole_curl_reset_callback(handle: PhpHandle<'_>, option: c_int);
    // The write, header or read slot's method and file, by its callable option.
    safe fn swoole_curl_method(handle: PhpHandle<'_>, stream: c_int) -> c_int;
    safe fn swoole_curl_set_method(handle: PhpHandle<'_>, stream: c_int, method: c_int);
    safe fn swoole_curl_file(handle: PhpHandle<'_>, stream: c_int) -> Option<StdioFile>;
    // `CURLOPT_INFILE`'s stream, or null.
    safe fn swoole_curl_read_stream(handle: PhpHandle<'_>, value: &mut Owned);
    // The response `CURLOPT_RETURNTRANSFER` collects. Bails out past the memory limit.
    safe fn swoole_curl_buffer_append(handle: PhpHandle<'_>, data: Data<'_>, length: usize)
    -> bool;
    // The collected response as `curl_exec` returns it, which the handle keeps until its
    // next transfer; false with none collected.
    safe fn swoole_curl_buffer_result(handle: PhpHandle<'_>, result: &mut Owned) -> bool;
    safe fn swoole_curl_buffer_free(handle: PhpHandle<'_>);
    // `_php_curl_cleanup_handle`: the last transfer's response, request header and error.
    safe fn swoole_curl_cleanup_handle(handle: PhpHandle<'_>);
    // Flushes the stream, when the value still is one. A wrapper's PHP code can run.
    safe fn swoole_curl_stream_flush(value: &Owned);
    safe fn fflush(file: StdioFile) -> c_int;
    safe fn swoole_curl_record_header(handle: PhpHandle<'_>, data: Data<'_>, length: usize);
    safe fn swoole_curl_handle_value(handle: PhpHandle<'_>, value: &mut Owned);
    safe fn swoole_curl_set_in_callback(handle: PhpHandle<'_>, running: bool);
    safe fn swoole_curl_has_callable(handle: PhpHandle<'_>, option: c_int) -> bool;
    // Releases the option's callable and keeps `value` as it. From PHP 8.4 it resolves
    // `value` (an autoloader may run): null keeps none, and one that is not callable keeps
    // none and is false, with the reason in `error`.
    safe fn swoole_curl_set_callable(
        handle: PhpHandle<'_>,
        option: c_int,
        value: &Value,
        error: &mut Owned,
    ) -> bool;
    // Runs the option's callable, which the call holds. `retval` stays undefined when it
    // throws; false when PHP could not call it.
    safe fn swoole_curl_call(
        handle: PhpHandle<'_>,
        option: c_int,
        args: Zvals<'_>,
        retval: &mut Owned,
    ) -> bool;
    // Points libcurl's callback for the callable option at this module's.
    // With `installed` false the option goes back to libcurl's own default.
    safe fn swoole_curl_install_callback(handle: PhpHandle<'_>, option: c_int, installed: bool);
    safe fn swoole_curl_set_header_out(handle: PhpHandle<'_>, record: bool);
    // `PHPWRITE`, which runs an output handler's PHP code.
    safe fn swoole_curl_print(data: Data<'_>, length: usize);
    // A stream's `FILE *` can run its wrapper's PHP code.
    safe fn swoole_curl_fwrite(data: Data<'_>, size: usize, nmemb: usize, file: StdioFile)
    -> usize;
    safe fn swoole_curl_fread(data: Fill<'_>, size: usize, nmemb: usize, file: StdioFile) -> usize;
    safe fn swoole_curl_data_string(value: &mut Owned, data: Data<'_>, length: usize);
    // `length` is at most the string's and the buffer's.
    safe fn swoole_curl_data_fill(data: Fill<'_>, from: &ZStr, length: usize);
}

/// The body, the headers or the upload: each is its `CURLOPT_*FUNCTION`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Stream {
    Write = 20011,
    Header = 20079,
    Read = 20012,
}

impl Stream {
    pub(super) const fn of(option: zend_long) -> Option<Self> {
        match option {
            20011 => Some(Self::Write),
            20079 => Some(Self::Header),
            20012 => Some(Self::Read),
            _ => None,
        }
    }
}

/// The callables libcurl calls through an option of their own: each is its
/// `CURLOPT_*FUNCTION`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Plain {
    Progress = 20056,
    Xferinfo = 20219,
    Fnmatch = 20200,
    Debug = 20094,
    Prereq = 20312,
    SshHostKey = 20316,
}

impl Plain {
    /// The option's callable, where this PHP and libcurl have it, as the copies' `#if`s.
    pub(super) fn of(option: zend_long) -> Option<Self> {
        match option {
            20056 => Some(Self::Progress),
            20219 => Some(Self::Xferinfo),
            20200 => Some(Self::Fnmatch),
            20094 if PHP_VERSION_ID >= 80400 => Some(Self::Debug),
            20312 if PHP_VERSION_ID >= 80400 && libcurl(7, 80, 0) => Some(Self::Prereq),
            20316 if PHP_VERSION_ID >= 80300 && libcurl(7, 84, 0) => Some(Self::SshHostKey),
            _ => None,
        }
    }
}

/// A duplicated easy handle inherits libcurl's callback functions, but each
/// installed function must receive the copy as its userdata.
pub(super) fn copy_handlers(copy: PhpHandle<'_>, source: PhpHandle<'_>) {
    swoole_curl_copy_streams(copy, source);
    for stream in [Stream::Read, Stream::Write, Stream::Header] {
        swoole_curl_copy_callback(copy, source, stream as c_int, 0);
    }
    // Each pair is a CURLOPT_*FUNCTION and its corresponding CURLOPT_*DATA.
    // PROGRESSDATA is an alias for XFERINFODATA. The native slot lookup skips
    // callbacks absent from this PHP or libcurl build.
    for (callback, data) in [
        (Plain::Progress, 10057),
        (Plain::Xferinfo, 10057),
        (Plain::Fnmatch, 10202),
        (Plain::Debug, 10095),
        (Plain::Prereq, 10313),
        (Plain::SshHostKey, 10317),
    ] {
        swoole_curl_copy_callback(copy, source, callback as c_int, data);
    }
}

swoole_rs_zend::handlers! {
    zif_swoole_native_curl_exec => exec;
    zif_swoole_native_curl_multi_getcontent => getcontent;
}

/// Read the easy handle's return-transfer buffer without consuming it.
fn getcontent(call: &Call<'_>) {
    let Some(handle) = argument(call, 1) else {
        return;
    };
    if swoole_curl_method(handle, Stream::Write as c_int) != RETURN {
        call.set_null();
        return;
    }
    let mut body = Owned::default();
    if swoole_curl_buffer_result(handle, &mut body) {
        call.set_owned(body);
    } else {
        call.set_empty_string();
    }
}

/// `curl_exec`: runs the transfer in the handle's own multi. The response with
/// `CURLOPT_RETURNTRANSFER`, true without it, false when the transfer failed.
fn exec(call: &Call<'_>) {
    let Some(handle) = argument(call, 1) else {
        return;
    };
    call.set_bool(false);
    if !checked(handle, true) {
        return;
    }
    if !prepare_transfer(handle) {
        return;
    }
    let mut error = 0;
    // A callback's bailout leaves libcurl through here.
    let transfer = contain_with(|| {
        error = crate::swoole_curl_easy_perform(crate::Idle(swoole_curl_easy(handle)));
    });
    if transfer.is_err() {
        bailout::defer(transfer);
        return;
    }
    swoole_curl_save_error(handle, error);
    if error != 0 {
        swoole_curl_buffer_free(handle);
        return;
    }
    // A flushed stream's wrapper can set the handle's options, so each step reads the
    // write method afresh.
    let returns = || swoole_curl_method(handle, Stream::Write as c_int) == RETURN;
    let (mut returned, mut body) = (false, Owned::default());
    bailout::defer(contain_with(|| {
        swoole_curl_stream_flush(swoole_curl_file_stream(handle, CURLOPT_STDERR));
        returned = returns();
        if returned && swoole_curl_buffer_result(handle, &mut body) {
            return;
        }
        // What the transfer wrote to a file reaches the disk.
        for stream in [Stream::Write, Stream::Header] {
            let file = swoole_curl_file(handle, stream as c_int);
            if let (FILE, Some(file)) = (swoole_curl_method(handle, stream as c_int), file) {
                fflush(file);
            }
        }
        returned = returns();
    }));
    if !returned {
        call.set_bool(true);
    } else if body.kind() == IS_UNDEF {
        call.set_empty_string();
    } else {
        call.set_owned(body);
    }
}

/// Validate streams and reset the buffers and error state for a new transfer.
pub(super) fn prepare_transfer(handle: PhpHandle<'_>) -> bool {
    verify(handle, true);
    if bailout::pending() {
        return false;
    }
    swoole_curl_cleanup_handle(handle);
    true
}

/// `_php_curl_verify_handlers`: a stream closed under the handle is dropped, after a
/// warning when `report`, and its option goes back to the default.
pub(super) fn verify(handle: PhpHandle<'_>, report: bool) {
    let streams = [
        (
            CURLOPT_STDERR,
            c"CURLOPT_STDERR resource has gone away, resetting to stderr",
        ),
        (
            CURLOPT_INFILE,
            c"CURLOPT_INFILE resource has gone away, resetting to default",
        ),
        (
            CURLOPT_WRITEHEADER,
            c"CURLOPT_WRITEHEADER resource has gone away, resetting to default",
        ),
        (
            CURLOPT_FILE,
            c"CURLOPT_FILE resource has gone away, resetting to default",
        ),
    ];
    for (option, message) in streams {
        if !swoole_curl_stream_gone(swoole_curl_file_stream(handle, option)) {
            continue;
        }
        if report && !warn(E_WARNING, message) {
            return;
        }
        // Released once its slot is empty and the handle no longer uses its file.
        let stream = std::mem::take(swoole_curl_file_stream(handle, option));
        swoole_curl_stream_reset(handle, option);
        drop(stream);
    }
}

/// Restore the default stream methods and release the plain option callbacks.
pub(super) fn reset_options(handle: PhpHandle<'_>) {
    for option in [
        CURLOPT_FILE,
        CURLOPT_WRITEHEADER,
        CURLOPT_INFILE,
        CURLOPT_STDERR,
    ] {
        let stream = std::mem::take(swoole_curl_file_stream(handle, option));
        if option != CURLOPT_STDERR {
            swoole_curl_stream_reset(handle, option);
        }
        if option == CURLOPT_INFILE {
            swoole_curl_set_method(handle, Stream::Read as c_int, DIRECT);
        }
        drop(stream);
        if bailout::pending() {
            return;
        }
    }
    release_plain_callbacks(handle);
    super::setopt::defaults(handle);
}

pub(super) fn free_callbacks(handle: PhpHandle<'_>) {
    swoole_curl_buffer_free(handle);
    let streams = if PHP_VERSION_ID >= 80400 {
        [Stream::Write, Stream::Header, Stream::Read]
    } else {
        [Stream::Write, Stream::Read, Stream::Header]
    };
    for stream in streams {
        swoole_curl_reset_callback(handle, stream as c_int);
    }
    swoole_curl_free_streams(handle);
    release_plain_callbacks(handle);
}

fn release_plain_callbacks(handle: PhpHandle<'_>) {
    for callback in [
        Plain::Progress,
        Plain::Xferinfo,
        Plain::Fnmatch,
        Plain::Debug,
        Plain::Prereq,
        Plain::SshHostKey,
    ] {
        swoole_curl_reset_callback(handle, callback as c_int);
    }
}

/// `php_curl_set_callable_handler`: the option's callable is released, then `value` kept as
/// it. False after (from PHP 8.4) the `TypeError` of a value that is not callable; before
/// 8.4 any value is kept, and a call fails later.
fn assign(
    call: &Call<'_>,
    handle: PhpHandle<'_>,
    option: c_int,
    value: &Value,
    array: bool,
    name: &str,
) -> bool {
    let (mut kept, mut reason) = (false, Owned::default());
    bailout::defer(contain_with(|| {
        kept = swoole_curl_set_callable(handle, option, value, &mut reason);
    }));
    if !kept && !bailout::pending() && swoole_rs_zend::host::exception().is_null() {
        let reason = reason.str().map_or(&[][..], ZStr::bytes);
        let message = [
            b"must be a valid callback for option ",
            name.as_bytes(),
            b", ",
            reason,
        ]
        .concat();
        call.argument_type_error(
            if array { 2 } else { 3 },
            &CString::new(message).unwrap_or_default(),
        );
    }
    kept
}

/// `CURLOPT_WRITEFUNCTION`, `CURLOPT_HEADERFUNCTION` and `CURLOPT_READFUNCTION`. Without
/// a callable the stream goes back to its default method.
pub(super) fn set_io(
    call: &Call<'_>,
    handle: PhpHandle<'_>,
    stream: Stream,
    value: &Value,
    array: bool,
) -> bool {
    let (name, default) = match stream {
        Stream::Write => ("CURLOPT_WRITEFUNCTION", STDOUT),
        Stream::Header => ("CURLOPT_HEADERFUNCTION", IGNORE),
        Stream::Read => ("CURLOPT_READFUNCTION", DIRECT),
    };
    let option = stream as c_int;
    let kept = assign(call, handle, option, value, array, name);
    if kept && swoole_curl_has_callable(handle, option) {
        swoole_curl_set_method(handle, option, USER);
        swoole_curl_save_error(handle, 0);
    } else {
        swoole_curl_set_method(handle, option, default);
    }
    kept
}

/// The callable options libcurl has a callback for, which is installed with them.
pub(super) fn set_plain(
    call: &Call<'_>,
    handle: PhpHandle<'_>,
    which: Plain,
    value: &Value,
    array: bool,
) -> bool {
    let name = match which {
        Plain::Progress => "CURLOPT_PROGRESSFUNCTION",
        Plain::Xferinfo => "CURLOPT_XFERINFOFUNCTION",
        Plain::Fnmatch => "CURLOPT_FNMATCH_FUNCTION",
        Plain::Debug => "CURLOPT_DEBUGFUNCTION",
        Plain::Prereq => "CURLOPT_PREREQFUNCTION",
        Plain::SshHostKey => "CURLOPT_SSH_HOSTKEYFUNCTION",
    };
    if !assign(call, handle, which as c_int, value, array, name) {
        return false;
    }
    // From 8.4 an option left without a callable gets libcurl's default back; before 8.4 the
    // callback is installed whatever was stored.
    let installed = PHP_VERSION_ID < 80400 || swoole_curl_has_callable(handle, which as c_int);
    swoole_curl_install_callback(handle, which as c_int, installed);
    swoole_curl_save_error(handle, 0);
    true
}

/// `CURLINFO_HEADER_OUT` as an option: whether the request header is recorded.
pub(super) fn set_header_out(handle: PhpHandle<'_>, record: bool) -> bool {
    if swoole_curl_has_callable(handle, Plain::Debug as c_int) {
        swoole_rs_zend::value_error(
            c"CURLINFO_HEADER_OUT option must not be set when the CURLOPT_DEBUGFUNCTION option is set",
        );
        return false;
    }
    swoole_curl_set_header_out(handle, record);
    swoole_curl_save_error(handle, 0);
    true
}

swoole_rs_zend::exports! {
/// `_php_curl_verify_handlers`, for `curl_exec`, the multi functions and the handle's free.
#[unsafe(no_mangle)]
extern "C" fn swoole_curl_verify_handlers(handle: PhpHandle<'_>, report: bool) {
    verify(handle, report);
}

/// `CURLOPT_WRITEFUNCTION`'s callback: the body.
#[unsafe(no_mangle)]
extern "C" fn swoole_curl_on_write(
    data: Data<'_>,
    size: usize,
    nmemb: usize,
    handle: PhpHandle<'_>,
) -> usize {
    let length = size.wrapping_mul(nmemb);
    match swoole_curl_method(handle, Stream::Write as c_int) {
        STDOUT => print(data, length),
        FILE => return write_file(handle, Stream::Write, data, size, nmemb),
        RETURN => return append(handle, data, length),
        USER => {
            let failed = c"Could not call the CURLOPT_WRITEFUNCTION";
            return write_user(handle, Stream::Write, data, length, failed);
        }
        _ => {}
    }
    length
}

/// `CURLOPT_HEADERFUNCTION`'s callback: each header line.
#[unsafe(no_mangle)]
extern "C" fn swoole_curl_on_header(
    data: Data<'_>,
    size: usize,
    nmemb: usize,
    handle: PhpHandle<'_>,
) -> usize {
    let length = size.wrapping_mul(nmemb);
    match swoole_curl_method(handle, Stream::Header as c_int) {
        IGNORE => {}
        // `CURLOPT_HEADER`: the headers go where the body goes.
        STDOUT if swoole_curl_method(handle, Stream::Write as c_int) == RETURN => {
            return append(handle, data, length);
        }
        STDOUT => print(data, length),
        FILE => return write_file(handle, Stream::Header, data, size, nmemb),
        USER => {
            let failed = c"Could not call the CURLOPT_HEADERFUNCTION";
            return write_user(handle, Stream::Header, data, length, failed);
        }
        _ => return usize::MAX,
    }
    length
}

/// `CURLOPT_READFUNCTION`'s callback: the upload.
#[unsafe(no_mangle)]
extern "C" fn swoole_curl_on_read(
    data: Fill<'_>,
    size: usize,
    nmemb: usize,
    handle: PhpHandle<'_>,
) -> usize {
    let file = swoole_curl_file(handle, Stream::Read as c_int);
    match (swoole_curl_method(handle, Stream::Read as c_int), file) {
        (DIRECT, Some(file)) => {
            let mut length = 0;
            bailout::defer(contain_with(|| {
                length = swoole_curl_fread(data, size, nmemb, file);
            }));
            length
        }
        (USER, _) => read_user(handle, data, size.wrapping_mul(nmemb)),
        _ => 0,
    }
}

/// `CURLOPT_PROGRESSFUNCTION`'s callback: nonzero aborts the transfer.
#[unsafe(no_mangle)]
extern "C" fn swoole_curl_on_progress(
    handle: PhpHandle<'_>,
    dltotal: f64,
    dlnow: f64,
    ultotal: f64,
    ulnow: f64,
) -> Transfer {
    let counts = [dltotal, dlnow, ultotal, ulnow].map(|count| Owned::long(count.c_cast()));
    let failed = c"Cannot call the CURLOPT_PROGRESSFUNCTION";
    progress(handle, Plain::Progress, counts, failed)
}

/// `CURLOPT_XFERINFOFUNCTION`'s callback, as [`swoole_curl_on_progress`].
#[unsafe(no_mangle)]
extern "C" fn swoole_curl_on_xferinfo(
    handle: PhpHandle<'_>,
    dltotal: i64,
    dlnow: i64,
    ultotal: i64,
    ulnow: i64,
) -> Transfer {
    let counts = [dltotal, dlnow, ultotal, ulnow].map(Owned::long);
    let failed = c"Cannot call the CURLOPT_XFERINFOFUNCTION";
    progress(handle, Plain::Xferinfo, counts, failed)
}

/// `CURLOPT_FNMATCH_FUNCTION`'s callback: whether a wildcard pattern matches a name.
#[unsafe(no_mangle)]
extern "C" fn swoole_curl_on_fnmatch(
    handle: PhpHandle<'_>,
    pattern: Text<'_>,
    string: Text<'_>,
) -> Matched {
    let args = [object(handle), text(pattern), text(string)];
    match invoke(handle, Plain::Fnmatch as c_int, &args) {
        // libcurl takes 0 as a match and anything else as none.
        Called::Returned(retval) => match retval.long() {
            0 => Matched::Yes,
            1 => Matched::No,
            _ => Matched::Fail,
        },
        Called::Failed => {
            warn(E_WARNING, c"Cannot call the CURLOPT_FNMATCH_FUNCTION");
            Matched::Fail
        }
        Called::Nothing => Matched::Fail,
    }
}

/// `CURLOPT_DEBUGFUNCTION`'s callback, which also records the request header for
/// `CURLINFO_HEADER_OUT`.
#[unsafe(no_mangle)]
extern "C" fn swoole_curl_on_debug(
    _easy: Easy<'_>,
    kind: c_int,
    data: Data<'_>,
    size: usize,
    handle: PhpHandle<'_>,
) -> Debugged {
    if kind == HEADER_OUT {
        swoole_curl_record_header(handle, data, size);
    }
    if swoole_curl_has_callable(handle, Plain::Debug as c_int) {
        let args = [object(handle), Owned::long(kind.into()), string(data, size)];
        // The debug callback's return value is ignored.
        _ = call(handle, Plain::Debug as c_int, &args, false);
    }
    Debugged::Done
}

/// `CURLOPT_PREREQFUNCTION`'s callback: whether the request goes ahead on its connection.
#[unsafe(no_mangle)]
extern "C" fn swoole_curl_on_prereq(
    handle: PhpHandle<'_>,
    primary_ip: Text<'_>,
    local_ip: Text<'_>,
    primary_port: c_int,
    local_port: c_int,
) -> Prereq {
    let args = [
        object(handle),
        text(primary_ip),
        text(local_ip),
        Owned::long(primary_port.into()),
        Owned::long(local_port.into()),
    ];
    let Called::Returned(retval) = invoke(handle, Plain::Prereq as c_int, &args) else {
        return Prereq::Ok;
    };
    let message = c"The CURLOPT_PREREQFUNCTION callback must return either CURL_PREREQFUNC_OK or CURL_PREREQFUNC_ABORT";
    match retval.as_long() {
        Some(0) => Prereq::Ok,
        Some(1) => Prereq::Abort,
        Some(_) => {
            swoole_rs_zend::value_error(message);
            Prereq::Ok
        }
        None => {
            swoole_rs_zend::type_error(message);
            Prereq::Ok
        }
    }
}

/// `CURLOPT_SSH_HOSTKEYFUNCTION`'s callback: whether the server's key is accepted.
#[unsafe(no_mangle)]
extern "C" fn swoole_curl_on_ssh_hostkey(
    handle: PhpHandle<'_>,
    keytype: c_int,
    key: Data<'_>,
    keylen: usize,
) -> HostKey {
    let args = [
        object(handle),
        Owned::long(keytype.into()),
        string(key, keylen),
        Owned::long(keylen.c_cast()),
    ];
    match invoke(handle, Plain::SshHostKey as c_int, &args) {
        Called::Returned(retval) => match retval.as_long() {
            Some(0) => HostKey::Ok,
            Some(1) => HostKey::Mismatch,
            _ => {
                swoole_rs_zend::throw_error(c"The CURLOPT_SSH_HOSTKEYFUNCTION callback must return either CURLKHMATCH_OK or CURLKHMATCH_MISMATCH");
                HostKey::Mismatch
            }
        },
        Called::Failed => {
            warn(E_WARNING, c"Cannot call the CURLOPT_SSH_HOSTKEYFUNCTION");
            HostKey::Mismatch
        }
        Called::Nothing => HostKey::Mismatch,
    }
}
}

/// `CURL_FNMATCHFUNC_MATCH`, `CURL_FNMATCHFUNC_NOMATCH` and `CURL_FNMATCHFUNC_FAIL`.
#[repr(i32)]
#[derive(Clone, Copy)]
enum Matched {
    Yes = 0,
    No = 1,
    Fail = 2,
}

/// What a progress callback tells libcurl.
#[repr(i32)]
#[derive(Clone, Copy)]
enum Transfer {
    Continue = 0,
    Abort = 1,
}

/// A debug callback's result: libcurl takes only 0.
#[repr(i32)]
#[derive(Clone, Copy)]
enum Debugged {
    Done = 0,
}

/// `CURL_PREREQFUNC_OK` and `CURL_PREREQFUNC_ABORT`.
#[repr(i32)]
#[derive(Clone, Copy)]
enum Prereq {
    Ok = 0,
    Abort = 1,
}

/// `CURLKHMATCH_OK` and `CURLKHMATCH_MISMATCH`.
#[repr(i32)]
#[derive(Clone, Copy)]
enum HostKey {
    Ok = 0,
    Mismatch = 1,
}

/// What a callable left.
enum Called {
    /// PHP could not call it (before PHP 8.4).
    Failed,
    /// It has none, threw or bailed out: the callback answers as if it was never set.
    Nothing,
    Returned(Owned),
}

/// Runs the option's callable: None when PHP could not call it, else what it returned,
/// undefined when it threw or bailed out (which is deferred). Without `keep` the return
/// value is released while the handle is still marked in-callback, so a destructor it runs
/// can't close or reset the handle; what comes back is then undefined.
fn call(handle: PhpHandle<'_>, option: c_int, args: &[Owned], keep: bool) -> Option<Owned> {
    let (mut called, mut retval) = (true, Owned::default());
    swoole_curl_set_in_callback(handle, true);
    bailout::defer(contain_with(|| {
        called = swoole_curl_call(handle, option, Zvals::owned(args), &mut retval);
        if !keep {
            drop(std::mem::take(&mut retval));
            // A destructor that bailed out is deferred: keep the handle marked, as below.
            if bailout::pending() {
                return;
            }
        }
        // A fatal bailout skips this reset: shutdown handlers still see the active callback.
        swoole_curl_set_in_callback(handle, false);
    }));
    called.then_some(retval)
}

/// [`call`] of an option that may have no callable, then the streams checked again as
/// [`verify`]: the call may have closed one.
fn invoke(handle: PhpHandle<'_>, option: c_int, args: &[Owned]) -> Called {
    if !swoole_curl_has_callable(handle, option) {
        return Called::Nothing;
    }
    match call(handle, option, args, true) {
        None => Called::Failed,
        Some(retval) if retval.kind() == IS_UNDEF => Called::Nothing,
        Some(retval) => {
            verify(handle, true);
            Called::Returned(retval)
        }
    }
}

/// The `CurlHandle`, as each callable's first argument.
fn object(handle: PhpHandle<'_>) -> Owned {
    let mut value = Owned::default();
    swoole_curl_handle_value(handle, &mut value);
    value
}

fn string(data: Data<'_>, length: usize) -> Owned {
    let mut value = Owned::default();
    swoole_curl_data_string(&mut value, data, length);
    value
}

fn text(text: Text<'_>) -> Owned {
    Owned::string(ZString::new(text.get().to_bytes()))
}

fn print(data: Data<'_>, length: usize) {
    bailout::defer(contain_with(|| swoole_curl_print(data, length)));
}

fn append(handle: PhpHandle<'_>, data: Data<'_>, length: usize) -> usize {
    let mut written = 0;
    if length > 0 {
        bailout::defer(contain_with(|| {
            if swoole_curl_buffer_append(handle, data, length) {
                written = length;
            }
        }));
    }
    written
}

fn write_file(
    handle: PhpHandle<'_>,
    stream: Stream,
    data: Data<'_>,
    size: usize,
    nmemb: usize,
) -> usize {
    let (Some(file), mut written) = (swoole_curl_file(handle, stream as c_int), 0) else {
        return 0;
    };
    bailout::defer(contain_with(|| {
        written = swoole_curl_fwrite(data, size, nmemb, file);
    }));
    written
}

/// The write or header callable: how many bytes it took.
fn write_user(
    handle: PhpHandle<'_>,
    stream: Stream,
    data: Data<'_>,
    length: usize,
    failed: &CStr,
) -> usize {
    let args = [object(handle), string(data, length)];
    match invoke(handle, stream as c_int, &args) {
        Called::Returned(retval) => retval.long().c_cast(),
        Called::Failed => {
            warn(E_WARNING, failed);
            usize::MAX
        }
        Called::Nothing => length,
    }
}

/// The read callable: how many bytes it put in libcurl's buffer of `size`.
fn read_user(handle: PhpHandle<'_>, data: Fill<'_>, size: usize) -> usize {
    let mut stream = Owned::default();
    swoole_curl_read_stream(handle, &mut stream);
    let args = [object(handle), stream, Owned::long(size.c_cast())];
    match invoke(handle, Stream::Read as c_int, &args) {
        Called::Returned(retval) => {
            if let Some(string) = retval.str() {
                let length = size.min(string.bytes().len());
                swoole_curl_data_fill(data, string, length);
                length
            } else {
                retval.as_long().map_or(0, CCast::c_cast)
            }
        }
        Called::Failed => {
            warn(E_WARNING, c"Cannot call the CURLOPT_READFUNCTION");
            READ_ABORT
        }
        Called::Nothing => 0,
    }
}

/// The progress callables: whether to abort the transfer, which any return value but 0 does.
fn progress(handle: PhpHandle<'_>, which: Plain, counts: [Owned; 4], failed: &CStr) -> Transfer {
    let [dltotal, dlnow, ultotal, ulnow] = counts;
    let args = [object(handle), dltotal, dlnow, ultotal, ulnow];
    match invoke(handle, which as c_int, &args) {
        Called::Returned(retval) => {
            if retval.long() == 0 {
                Transfer::Continue
            } else {
                Transfer::Abort
            }
        }
        Called::Nothing => Transfer::Continue,
        Called::Failed => {
            warn(E_WARNING, failed);
            Transfer::Continue
        }
    }
}
