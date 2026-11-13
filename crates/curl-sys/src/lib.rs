//! The libcurl calls and constants Swoole's coroutine cURL (`--enable-swoole-curl`)
//! uses, as `curl/curl.h` and `curl/multi.h` declare them (libcurl 7.56 and later,
//! which `config.m4` requires). It links the system libcurl, as the extension does.
#![expect(
    non_camel_case_types,
    reason = "the C type names, as curl.h spells them"
)]

use std::ffi::{c_int, c_long, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;

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
/// A multi handle `curl_multi_init` made, live until the unsafe `curl_multi_cleanup`
/// of it, whose caller drops the owner unused. It is not `Send`: libcurl leaves a
/// handle to one thread at a time.
#[repr(transparent)]
pub struct MultiHandle(NonNull<CURLM>);

impl MultiHandle {
    /// The handle, for the calls that stay unsafe and for native code.
    #[must_use]
    pub const fn as_ptr(&self) -> *mut CURLM {
        self.0.as_ptr()
    }

    /// The handle while its owner is borrowed, for the safe calls.
    #[must_use]
    pub const fn live(&self) -> LiveMulti<'_> {
        LiveMulti(self.0, PhantomData)
    }
}

/// A [`MultiHandle`], borrowed: what a safe libcurl call takes.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct LiveMulti<'a>(NonNull<CURLM>, PhantomData<&'a MultiHandle>);

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

#[link(name = "curl")]
unsafe extern "C" {
    pub fn curl_easy_init() -> *mut CURL;
    pub fn curl_easy_cleanup(curl: *mut CURL);
    pub fn curl_easy_reset(curl: *mut CURL);
    pub fn curl_easy_setopt(curl: *mut CURL, option: CURLoption, ...) -> CURLcode;
    pub fn curl_easy_getinfo(curl: *mut CURL, info: CURLINFO, ...) -> CURLcode;
    /// A new multi handle, or none: it takes nothing and touches no caller state.
    pub safe fn curl_multi_init() -> Option<MultiHandle>;
    pub fn curl_multi_cleanup(multi: *mut CURLM) -> CURLMcode;
    pub fn curl_multi_setopt(multi: *mut CURLM, option: CURLMoption, ...) -> CURLMcode;
    pub fn curl_multi_add_handle(multi: *mut CURLM, easy: *mut CURL) -> CURLMcode;
    pub fn curl_multi_remove_handle(multi: *mut CURLM, easy: *mut CURL) -> CURLMcode;
    pub fn curl_multi_assign(
        multi: *mut CURLM,
        sockfd: curl_socket_t,
        sockp: *mut c_void,
    ) -> CURLMcode;
    /// How long to wait before calling the handle again, into `milliseconds`: it reads
    /// the handle's timeouts and runs no callback. `CURLM_BAD_HANDLE` without a handle.
    pub safe fn curl_multi_timeout(
        multi: Option<LiveMulti<'_>>,
        milliseconds: &mut c_long,
    ) -> CURLMcode;
    pub fn curl_multi_socket_action(
        multi: *mut CURLM,
        s: curl_socket_t,
        ev_bitmask: c_int,
        running_handles: *mut c_int,
    ) -> CURLMcode;
    /// The next message of the handle's queue, which libcurl keeps until the handle's
    /// next call, or null (as without a handle): it pops the queue and runs no callback.
    pub safe fn curl_multi_info_read(
        multi: Option<LiveMulti<'_>>,
        msgs_in_queue: &mut c_int,
    ) -> *mut CURLMsg;
}
