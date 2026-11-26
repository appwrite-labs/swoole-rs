//! Reactor and socket state owned by the core runtime.
#[cfg(unix)]
mod wait_loop;
#[cfg(unix)]
pub use readiness::UringOps;
pub use wait_loop::*;
#[cfg(unix)]
mod poll;
#[cfg(unix)]
pub use poll::*;
#[cfg(unix)]
mod write;
#[cfg(unix)]
pub use write::*;
#[cfg(unix)]
mod change;
#[cfg(unix)]
mod registration;
#[cfg(unix)]
pub use change::*;
#[cfg(unix)]
mod registry;
#[cfg(unix)]
pub use registry::*;

#[cfg(unix)]
mod file_output;
#[cfg(unix)]
mod output;
#[cfg(unix)]
pub use file_output::*;
#[cfg(unix)]
pub use output::*;

mod deferred;
pub use deferred::*;

mod socket;
pub use socket::{Heap, KeptSockets, OwnedSocket, Socket, SocketLoans};

mod callbacks;
pub use callbacks::*;

pub(crate) mod signals;
pub use signals::{Signals, set_signals};

mod handlers;
pub use handlers::*;

mod wait;
pub use wait::*;

mod flush;
pub use flush::*;

mod events;
pub use events::*;

mod socket_timeouts;
pub use socket_timeouts::*;

#[cfg(unix)]
mod socket_options;
#[cfg(unix)]
pub use socket_options::*;
#[cfg(unix)]
mod socket_io;
#[cfg(unix)]
pub use socket_io::*;
#[cfg(unix)]
mod lent;
#[cfg(unix)]
pub use crate::lent::{Drain, Fill};
#[cfg(unix)]
mod socket_rw;
#[cfg(unix)]
pub use socket_rw::*;
#[cfg(unix)]
mod socket_sync;
#[cfg(unix)]
pub use socket_sync::*;
#[cfg(unix)]
mod socket_types;
#[cfg(unix)]
pub use socket_types::*;
#[cfg(unix)]
mod socket_methods;
#[cfg(unix)]
pub use socket_methods::*;
#[cfg(unix)]
mod socket_record;
#[cfg(unix)]
pub use socket_record::*;
#[cfg(unix)]
pub mod event_api;
#[cfg(unix)]
pub mod reactor_upstream;
#[cfg(unix)]
pub mod readiness;
#[cfg(unix)]
pub mod socket_upstream;

mod iov_progress;
pub use iov_progress::*;

#[cfg(unix)]
mod iov_owned;
#[cfg(unix)]
pub use iov_owned::*;

#[cfg(unix)]
mod reactor_state;
#[cfg(unix)]
pub use reactor_state::*;

#[cfg(test)]
mod isolated_process {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/isolated_process.rs"
    ));
}
