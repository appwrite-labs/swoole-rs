// SPDX-License-Identifier: Apache-2.0
//! Upstream's TLS identifiers. No C header exports them; the `tls-constants` gate
//! compares them with the pinned `swoole_ssl.h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(non_camel_case_types, reason = "upstream's identifiers")]
pub enum SslCreateFlag {
    SW_SSL_SERVER = 1,
    SW_SSL_CLIENT = 2,
}
pub use swoole_rs_corebase::constants::SslState;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SslVersion(u32);

#[expect(non_upper_case_globals, reason = "upstream protocol identifiers")]
impl SslVersion {
    pub const SW_SSL_SSLv2: Self = Self(1 << 1);
    pub const SW_SSL_SSLv3: Self = Self(1 << 2);
    pub const SW_SSL_TLSv1: Self = Self(1 << 3);
    pub const SW_SSL_TLSv1_1: Self = Self(1 << 4);
    pub const SW_SSL_TLSv1_2: Self = Self(1 << 5);
    pub const SW_SSL_TLSv1_3: Self = Self(1 << 6);
    pub const SW_SSL_DTLS: Self = Self(1 << 7);
}

swoole_rs_corebase::impl_flags!(SslVersion, u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(non_camel_case_types, reason = "upstream's identifiers")]
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
pub const SW_SSL_ALL: SslVersion = SslVersion::SW_SSL_SSLv2
    .union(SslVersion::SW_SSL_SSLv3)
    .union(SslVersion::SW_SSL_TLSv1)
    .union(SslVersion::SW_SSL_TLSv1_1)
    .union(SslVersion::SW_SSL_TLSv1_2)
    .union(SslVersion::SW_SSL_TLSv1_3);
