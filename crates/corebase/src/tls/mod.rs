// SPDX-License-Identifier: Apache-2.0
//! TLS, DTLS and socket I/O over OpenSSL, owned by the core runtime.
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
