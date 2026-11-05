//! HTTP CONNECT construction reads the canonical Rust proxy owner directly.
use crate::{
    HttpProxyRequest, HttpProxyText, swr_v1_http_proxy_pack, swr_v2_http_proxy_authorization,
};
use swoole_rs_corebase::ProxyState;

/// # Safety
/// owner is live/stable; output lends512 bytes and written is writable/disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_proxy_owned_authorization(
    owner: *const ProxyState,
    output: *mut u8,
    written: *mut usize,
) -> i32 {
    // SAFETY: caller lends the live owner's unchanged string slots.
    let owner = unsafe { &*owner };
    // SAFETY: the username is stable through the bounded codec.
    let user = unsafe { owner.string(1) };
    // SAFETY: the password is stable through the bounded codec.
    let password = unsafe { owner.string(2) };
    let credentials = [
        HttpProxyText {
            data: user.data.cast(),
            length: user.length,
        },
        HttpProxyText {
            data: password.data.cast(),
            length: password.length,
        },
    ];
    // SAFETY: stable credentials and exclusive512-byte output/length match the codec contract.
    unsafe { swr_v2_http_proxy_authorization(credentials.as_ptr(), output, written) }
}
/// # Safety
/// owner/input spans remain stable; output and written are exclusive/disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_proxy_owned_pack(
    owner: *const ProxyState,
    host: HttpProxyText,
    version: HttpProxyText,
    authorization: HttpProxyText,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    // SAFETY: caller lends a live stable owner for request construction.
    let owner = unsafe { &*owner };
    // SAFETY: leaf scalar snapshot before formatting.
    let fields = unsafe { *owner.fields.get() };
    // SAFETY: the target bytes remain stable throughout formatting.
    let target = unsafe { owner.string(3) };
    // SAFETY: the password slot remains stable throughout formatting.
    let password = unsafe { owner.string(2) };
    let request = HttpProxyRequest {
        target: HttpProxyText {
            data: target.data.cast(),
            length: target.length,
        },
        host,
        version,
        authorization,
        port: fields.target_port,
        authenticated: u32::from(password.length != 0),
    };
    // SAFETY: unchanged owner/spans and disjoint exclusive output follow the codec contract.
    unsafe { swr_v1_http_proxy_pack(&raw const request, output, capacity, written) }
}
