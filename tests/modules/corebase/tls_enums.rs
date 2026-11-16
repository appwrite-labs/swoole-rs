// SPDX-License-Identifier: Apache-2.0
//! The TLS identifiers, which no C header exports, as `NAME VALUE` lines that
//! `constants.py` compares with the pinned `swoole_ssl.h`.
use swoole_rs_tls::{SW_SSL_ALL, SslCreateFlag, SslMethod, SslVersion};

include!("enum_table.rs");

#[expect(clippy::print_stdout, reason = "constants.py reads the table")]
fn main() {
    enum_table! {
        SslCreateFlag { SW_SSL_SERVER, SW_SSL_CLIENT }
        SslVersion {
            SW_SSL_SSLv2, SW_SSL_SSLv3, SW_SSL_TLSv1, SW_SSL_TLSv1_1, SW_SSL_TLSv1_2,
            SW_SSL_TLSv1_3, SW_SSL_DTLS,
        }
        SslMethod {
            SW_SSLv23_METHOD, SW_SSLv3_METHOD, SW_SSLv3_SERVER_METHOD, SW_SSLv3_CLIENT_METHOD,
            SW_SSLv23_SERVER_METHOD, SW_SSLv23_CLIENT_METHOD, SW_TLSv1_METHOD,
            SW_TLSv1_SERVER_METHOD, SW_TLSv1_CLIENT_METHOD, SW_TLSv1_1_METHOD,
            SW_TLSv1_1_SERVER_METHOD, SW_TLSv1_1_CLIENT_METHOD, SW_TLSv1_2_METHOD,
            SW_TLSv1_2_SERVER_METHOD, SW_TLSv1_2_CLIENT_METHOD, SW_DTLS_CLIENT_METHOD,
            SW_DTLS_SERVER_METHOD,
        }
    }
    println!("SW_SSL_ALL {SW_SSL_ALL}");
}
