//! The libcurl calls and constants Swoole's coroutine cURL (`--enable-swoole-curl`)
//! uses, as `curl/curl.h` and `curl/multi.h` declare them (libcurl 7.56 and later,
//! which `config.m4` requires). It links the system libcurl, as the extension does.
#![expect(
    non_camel_case_types,
    reason = "the C type names, as curl.h spells them"
)]

use std::ffi::{c_int, c_long, c_void};

/// `CURL`, only ever passed by pointer.
#[repr(C)]
pub struct CURL {
    _opaque: [u8; 0],
}
/// `CURLM`, only ever passed by pointer.
#[repr(C)]
pub struct CURLM {
    _opaque: [u8; 0],
}
/// `curl_socket_t` on POSIX.
pub type curl_socket_t = c_int;
pub type CURLcode = c_int;
pub type CURLMcode = c_int;
pub type CURLoption = c_int;
pub type CURLMoption = c_int;
pub type CURLINFO = c_int;

pub const CURLE_OK: CURLcode = 0;
pub const CURLE_FAILED_INIT: CURLcode = 2;
pub const CURLE_ABORTED_BY_CALLBACK: CURLcode = 42;
pub const CURLM_OK: CURLMcode = 0;
pub const CURLM_INTERNAL_ERROR: CURLMcode = 4;

/// `CURLOPTTYPE_OBJECTPOINT + 103`.
pub const CURLOPT_PRIVATE: CURLoption = 10103;
/// `CURLOPTTYPE_STRINGPOINT + 2`.
pub const CURLOPT_URL: CURLoption = 10002;
/// `CURLINFO_STRING + 21`.
pub const CURLINFO_PRIVATE: CURLINFO = 0x0010_0000 + 21;
/// `CURLOPTTYPE_FUNCTIONPOINT + 1`.
pub const CURLMOPT_SOCKETFUNCTION: CURLMoption = 20001;
/// `CURLOPTTYPE_OBJECTPOINT + 2`.
pub const CURLMOPT_SOCKETDATA: CURLMoption = 10002;
/// `CURLOPTTYPE_FUNCTIONPOINT + 4`.
pub const CURLMOPT_TIMERFUNCTION: CURLMoption = 20004;
/// `CURLOPTTYPE_OBJECTPOINT + 5`.
pub const CURLMOPT_TIMERDATA: CURLMoption = 10005;

pub const CURL_POLL_IN: c_int = 1;
pub const CURL_POLL_OUT: c_int = 2;
pub const CURL_POLL_INOUT: c_int = 3;
pub const CURL_POLL_REMOVE: c_int = 4;
/// `CURL_SOCKET_BAD`.
pub const CURL_SOCKET_TIMEOUT: curl_socket_t = -1;
pub const CURL_CSELECT_IN: c_int = 1;
pub const CURL_CSELECT_OUT: c_int = 2;
pub const CURL_CSELECT_ERR: c_int = 4;
/// `CURLMSG_DONE`.
pub const CURLMSG_DONE: c_int = 1;

/// `CURLMsg`: `data` is a union of `void *whatever` and `CURLcode result`.
#[repr(C)]
pub struct CURLMsg {
    pub msg: c_int,
    pub easy_handle: *mut CURL,
    pub data: *mut c_void,
}

impl CURLMsg {
    /// `data.result`: the union's `CURLcode`, at its start.
    #[must_use]
    pub fn result(&self) -> CURLcode {
        let data = (&raw const self.data).cast::<CURLcode>();
        // SAFETY: the union's first bytes hold the CURLcode libcurl wrote.
        unsafe { data.read() }
    }
}

/// `curl_socket_callback`.
pub type curl_socket_callback = unsafe extern "C" fn(
    easy: *mut CURL,
    s: curl_socket_t,
    what: c_int,
    userp: *mut c_void,
    socketp: *mut c_void,
) -> c_int;
/// `curl_multi_timer_callback`.
pub type curl_multi_timer_callback =
    unsafe extern "C" fn(multi: *mut CURLM, timeout_ms: c_long, userp: *mut c_void) -> c_int;

#[link(name = "curl")]
unsafe extern "C" {
    pub fn curl_easy_init() -> *mut CURL;
    pub fn curl_easy_cleanup(curl: *mut CURL);
    pub fn curl_easy_reset(curl: *mut CURL);
    pub fn curl_easy_setopt(curl: *mut CURL, option: CURLoption, ...) -> CURLcode;
    pub fn curl_easy_getinfo(curl: *mut CURL, info: CURLINFO, ...) -> CURLcode;
    pub fn curl_multi_init() -> *mut CURLM;
    pub fn curl_multi_cleanup(multi: *mut CURLM) -> CURLMcode;
    pub fn curl_multi_setopt(multi: *mut CURLM, option: CURLMoption, ...) -> CURLMcode;
    pub fn curl_multi_add_handle(multi: *mut CURLM, easy: *mut CURL) -> CURLMcode;
    pub fn curl_multi_remove_handle(multi: *mut CURLM, easy: *mut CURL) -> CURLMcode;
    pub fn curl_multi_assign(
        multi: *mut CURLM,
        sockfd: curl_socket_t,
        sockp: *mut c_void,
    ) -> CURLMcode;
    pub fn curl_multi_timeout(multi: *mut CURLM, milliseconds: *mut c_long) -> CURLMcode;
    pub fn curl_multi_socket_action(
        multi: *mut CURLM,
        s: curl_socket_t,
        ev_bitmask: c_int,
        running_handles: *mut c_int,
    ) -> CURLMcode;
    pub fn curl_multi_info_read(multi: *mut CURLM, msgs_in_queue: *mut c_int) -> *mut CURLMsg;
}
