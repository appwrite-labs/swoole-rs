//! PHP signatures and registration order of Swoole's native cURL hooks.
use crate::{callbacks, getinfo, php_handle, php_multi, setopt};
use std::ptr::NonNull;
use swoole_rs_zend::arginfo::{
    ArgInfo, END, Entry, Methods, Slot, Table, arg, arg_class, arg_type, begin_class,
    begin_class_mask, begin_mask, begin_type, entry,
};
use swoole_rs_zend::sys::{
    _IS_BOOL, IS_ARRAY, IS_DOUBLE, IS_LONG, IS_MIXED, IS_STRING, IS_VOID, MAY_BE_ARRAY,
    MAY_BE_BOOL, MAY_BE_FALSE, MAY_BE_STRING, PHP_VERSION_ID, zend_function_entry,
};
use swoole_rs_zend::{Class, Minit, bailout};

const EASY: Slot = arg_class(c"handle", 0, false, c"CurlHandle", false, None);
const MULTI: Slot = arg_class(c"multi_handle", 0, false, c"CurlMultiHandle", false, None);
const OPTION: Slot = arg_type(c"option", 0, false, IS_LONG, false, None);
const VALUE: Slot = arg_type(c"value", 0, false, IS_MIXED, false, None);

macro_rules! function {
    ($name:literal, $handler:path, $($slot:expr),+) => {
        entry($name, $handler, &ArgInfo::new([$($slot),+]), 0)
    };
}

const CLOSE: Entry = function!(
    c"swoole_native_curl_close",
    php_handle::zif_swoole_native_curl_close,
    begin_type(1, 0, IS_VOID, false),
    EASY
);
const COPY_HANDLE: Entry = function!(
    c"swoole_native_curl_copy_handle",
    php_handle::zif_swoole_native_curl_copy_handle,
    begin_class_mask(1, 0, c"CurlHandle", MAY_BE_FALSE),
    EASY
);
const ERRNO: Entry = function!(
    c"swoole_native_curl_errno",
    php_handle::zif_swoole_native_curl_errno,
    begin_type(1, 0, IS_LONG, false),
    EASY
);
const ERROR: Entry = function!(
    c"swoole_native_curl_error",
    php_handle::zif_swoole_native_curl_error,
    begin_type(1, 0, IS_STRING, false),
    EASY
);
const EXEC: Entry = function!(
    c"swoole_native_curl_exec",
    callbacks::zif_swoole_native_curl_exec,
    begin_mask(1, 0, MAY_BE_STRING | MAY_BE_BOOL),
    EASY
);
const GETINFO: Entry = function!(
    c"swoole_native_curl_getinfo",
    getinfo::zif_swoole_native_curl_getinfo,
    begin_type(1, 0, IS_MIXED, false),
    EASY,
    arg_type(c"option", 0, false, IS_LONG, true, Some(c"null"))
);
const INIT: Entry = function!(
    c"swoole_native_curl_init",
    php_handle::zif_swoole_native_curl_init,
    begin_class_mask(0, 0, c"CurlHandle", MAY_BE_FALSE),
    arg_type(c"url", 0, false, IS_STRING, true, Some(c"null"))
);
const UPKEEP: Entry = function!(
    c"swoole_native_curl_upkeep",
    php_handle::zif_swoole_native_curl_upkeep,
    begin_type(1, 0, _IS_BOOL, false),
    EASY
);
const SETOPT: Entry = function!(
    c"swoole_native_curl_setopt",
    setopt::zif_swoole_native_curl_setopt,
    begin_type(3, 0, _IS_BOOL, false),
    EASY,
    OPTION,
    VALUE
);
const SETOPT_ARRAY: Entry = function!(
    c"swoole_native_curl_setopt_array",
    setopt::zif_swoole_native_curl_setopt_array,
    begin_type(2, 0, _IS_BOOL, false),
    EASY,
    arg_type(c"options", 0, false, IS_ARRAY, false, None)
);
const RESET: Entry = function!(
    c"swoole_native_curl_reset",
    php_handle::zif_swoole_native_curl_reset,
    begin_type(1, 0, IS_VOID, false),
    EASY
);
const ESCAPE: Entry = function!(
    c"swoole_native_curl_escape",
    php_handle::zif_swoole_native_curl_escape,
    begin_mask(2, 0, MAY_BE_STRING | MAY_BE_FALSE),
    EASY,
    arg_type(c"string", 0, false, IS_STRING, false, None)
);
const UNESCAPE: Entry = function!(
    c"swoole_native_curl_unescape",
    php_handle::zif_swoole_native_curl_unescape,
    begin_mask(2, 0, MAY_BE_STRING | MAY_BE_FALSE),
    EASY,
    arg_type(c"string", 0, false, IS_STRING, false, None)
);
const PAUSE: Entry = function!(
    c"swoole_native_curl_pause",
    php_handle::zif_swoole_native_curl_pause,
    begin_type(2, 0, IS_LONG, false),
    EASY,
    arg_type(c"flags", 0, false, IS_LONG, false, None)
);
const MULTI_ADD_HANDLE: Entry = function!(
    c"swoole_native_curl_multi_add_handle",
    php_multi::zif_swoole_native_curl_multi_add_handle,
    begin_type(2, 0, IS_LONG, false),
    MULTI,
    EASY
);
const MULTI_CLOSE: Entry = function!(
    c"swoole_native_curl_multi_close",
    php_multi::zif_swoole_native_curl_multi_close,
    begin_type(1, 0, IS_VOID, false),
    MULTI
);
const MULTI_ERRNO: Entry = function!(
    c"swoole_native_curl_multi_errno",
    php_multi::zif_swoole_native_curl_multi_errno,
    begin_type(1, 0, IS_LONG, false),
    MULTI
);
const MULTI_EXEC: Entry = function!(
    c"swoole_native_curl_multi_exec",
    php_multi::zif_swoole_native_curl_multi_exec,
    begin_type(2, 0, IS_LONG, false),
    MULTI,
    arg(c"still_running", 1, false, None)
);
const MULTI_SELECT: Entry = function!(
    c"swoole_native_curl_multi_select",
    php_multi::zif_swoole_native_curl_multi_select,
    begin_type(1, 0, IS_LONG, false),
    MULTI,
    arg_type(c"timeout", 0, false, IS_DOUBLE, false, Some(c"1.0"))
);
const MULTI_SETOPT: Entry = function!(
    c"swoole_native_curl_multi_setopt",
    php_multi::zif_swoole_native_curl_multi_setopt,
    begin_type(3, 0, _IS_BOOL, false),
    MULTI,
    OPTION,
    VALUE
);
const MULTI_GETCONTENT: Entry = function!(
    c"swoole_native_curl_multi_getcontent",
    callbacks::zif_swoole_native_curl_multi_getcontent,
    begin_type(1, 0, IS_STRING, true),
    EASY
);
const MULTI_INFO_READ: Entry = function!(
    c"swoole_native_curl_multi_info_read",
    php_multi::zif_swoole_native_curl_multi_info_read,
    begin_mask(1, 0, MAY_BE_ARRAY | MAY_BE_FALSE),
    MULTI,
    arg(c"queued_messages", 1, false, Some(c"null"))
);
const MULTI_INIT: Entry = function!(
    c"swoole_native_curl_multi_init",
    php_multi::zif_swoole_native_curl_multi_init,
    begin_class(0, 0, c"CurlMultiHandle", false)
);
const MULTI_REMOVE_HANDLE: Entry = function!(
    c"swoole_native_curl_multi_remove_handle",
    php_multi::zif_swoole_native_curl_multi_remove_handle,
    begin_type(2, 0, IS_LONG, false),
    MULTI,
    EASY
);

static FUNCTIONS: Methods<25> = Methods::new(if PHP_VERSION_ID < 80400 {
    [
        CLOSE,
        COPY_HANDLE,
        ERRNO,
        ERROR,
        EXEC,
        GETINFO,
        INIT,
        UPKEEP,
        SETOPT,
        SETOPT_ARRAY,
        RESET,
        ESCAPE,
        UNESCAPE,
        PAUSE,
        MULTI_ADD_HANDLE,
        MULTI_CLOSE,
        MULTI_ERRNO,
        MULTI_EXEC,
        MULTI_SELECT,
        MULTI_SETOPT,
        MULTI_GETCONTENT,
        MULTI_INFO_READ,
        MULTI_INIT,
        MULTI_REMOVE_HANDLE,
        END,
    ]
} else {
    [
        CLOSE,
        COPY_HANDLE,
        ERRNO,
        ERROR,
        ESCAPE,
        UNESCAPE,
        MULTI_SETOPT,
        EXEC,
        GETINFO,
        INIT,
        UPKEEP,
        MULTI_ADD_HANDLE,
        MULTI_CLOSE,
        MULTI_ERRNO,
        MULTI_EXEC,
        MULTI_GETCONTENT,
        MULTI_INFO_READ,
        MULTI_INIT,
        MULTI_REMOVE_HANDLE,
        MULTI_SELECT,
        PAUSE,
        RESET,
        SETOPT_ARRAY,
        SETOPT,
        END,
    ]
});

static EXCEPTION: Class = Class::new();

unsafe extern "C" {
    safe fn swoole_curl_startup(entries: NonNull<zend_function_entry>) -> bool;
}

/// Register the native hook functions and their exception during runtime MINIT.
pub fn minit(minit: &Minit) {
    let globals = swoole_rs_phpswoole::globals();
    if !globals.cli.get() {
        return;
    }
    let functions = if PHP_VERSION_ID >= 80200 && setopt::libcurl(7, 62, 0) {
        FUNCTIONS.table()
    } else {
        FUNCTIONS.without(&[c"swoole_native_curl_upkeep"])
    };
    let Some(entries) = NonNull::new(functions.as_ptr().cast_mut()) else {
        return;
    };
    if !swoole_curl_startup(entries) {
        return;
    }
    minit.register_child(
        &EXCEPTION,
        c"Swoole\\Coroutine\\Curl\\Exception",
        Table::NONE,
        minit.parent(swoole_rs_phpswoole::exception()),
        |_, _| {},
    );
    if globals.use_shortname.get() {
        bailout::defer(minit.alias(&EXCEPTION, c"Co\\Coroutine\\Curl\\Exception"));
    }
}
