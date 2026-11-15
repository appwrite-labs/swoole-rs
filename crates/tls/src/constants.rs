// SPDX-License-Identifier: Apache-2.0
//! OpenSSL 3 compatibility identifiers shared by Rust operations and native SDK.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(non_camel_case_types, reason = "preserve the native SDK names")]
pub enum SslCreateFlag {
    SW_SSL_SERVER = 1,
    SW_SSL_CLIENT = 2,
}
pub use swoole_rs_corebase::constants::SslState;
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(non_camel_case_types, reason = "preserve the native SDK names")]
pub enum SslVersion {
    SW_SSL_SSLv2 = 1 << 1,
    SW_SSL_SSLv3 = 1 << 2,
    SW_SSL_TLSv1 = 1 << 3,
    SW_SSL_TLSv1_1 = 1 << 4,
    SW_SSL_TLSv1_2 = 1 << 5,
    SW_SSL_TLSv1_3 = 1 << 6,
    SW_SSL_DTLS = 1 << 7,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(non_camel_case_types, reason = "preserve the native SDK names")]
pub enum SslMethod {
    SW_SSLv23_METHOD = 0,
    SW_SSLv3_METHOD = 1,
    SW_SSLv3_SERVER_METHOD = 2,
    SW_SSLv3_CLIENT_METHOD = 3,
    SW_SSLv23_SERVER_METHOD = 4,
    SW_SSLv23_CLIENT_METHOD = 5,
    SW_TLSv1_METHOD = 6,
    SW_TLSv1_SERVER_METHOD = 7,
    SW_TLSv1_CLIENT_METHOD = 8,
    SW_TLSv1_1_METHOD = 9,
    SW_TLSv1_1_SERVER_METHOD = 10,
    SW_TLSv1_1_CLIENT_METHOD = 11,
    SW_TLSv1_2_METHOD = 12,
    SW_TLSv1_2_SERVER_METHOD = 13,
    SW_TLSv1_2_CLIENT_METHOD = 14,
    SW_DTLS_CLIENT_METHOD = 15,
    SW_DTLS_SERVER_METHOD = 16,
}
pub const SW_SSL_ALL: i32 = SslVersion::SW_SSL_SSLv2 as i32
    | SslVersion::SW_SSL_SSLv3 as i32
    | SslVersion::SW_SSL_TLSv1 as i32
    | SslVersion::SW_SSL_TLSv1_1 as i32
    | SslVersion::SW_SSL_TLSv1_2 as i32
    | SslVersion::SW_SSL_TLSv1_3 as i32;
