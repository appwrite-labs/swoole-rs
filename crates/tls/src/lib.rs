#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
// SPDX-License-Identifier: Apache-2.0
//! TLS and DTLS over OpenSSL (openssl-sys): `SSLContext`'s context creation and
//! configuration (`context`), `network::Socket`'s TLS I/O (`socket`, called from
//! reactor's patch), and the DTLS BIO's read/control/listen decisions.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
#[cfg(unix)]
mod context;
#[cfg(unix)]
pub mod event_socket;
#[cfg(unix)]
mod handle;
#[cfg(unix)]
pub mod reactor_write;
#[cfg(unix)]
mod socket;
#[cfg(unix)]
pub mod socket_pair;
#[cfg(unix)]
pub mod upstream;
#[cfg(unix)]
pub use context::*;
#[cfg(unix)]
pub use handle::*;
#[cfg(unix)]
pub use socket::*;

#[cfg(unix)]
mod owned;
#[cfg(unix)]
pub use owned::*;

#[cfg(unix)]
mod info;

mod constants;
pub use constants::*;

#[cfg(unix)]
mod dtls;
#[cfg(unix)]
pub use dtls::*;
