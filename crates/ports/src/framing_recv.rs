// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/protocol/base.cc (Swoole contributors).
//! Port-specific packet dispatch and pause state.
use core::sync::atomic::Ordering;
use swoole_rs_clients::framing_recv::Source;
use swoole_rs_clients::protocol::{Package, Protocol};
use swoole_rs_corebase::DataHead;
use swoole_rs_server::port_config::PortConfig;

pub use swoole_rs_clients::framing_recv::{default_length_func, swoole_hton64, swoole_ntoh64};

/// Whether this port has a PHP length callable, sampled once per receive.
#[derive(Clone, Copy)]
pub(crate) struct PortSource<'a>(pub(crate) &'a PortConfig, pub(crate) bool);

impl Source for PortSource<'_> {
    fn protocol(self) -> Protocol {
        self.0.protocol.get()
    }
    fn address(self) -> *const Protocol {
        self.0.protocol.as_ptr()
    }
    fn length_owner(self) -> Option<std::rc::Rc<dyn core::any::Any>> {
        if self.1 { self.0.length_owner() } else { None }
    }
}

/// `package` as the payload a dispatch with the head `info` sends: none (a null one)
/// when the package is.
pub(crate) fn send_data<'a>(
    package: &'a mut Package<'_>,
    info: DataHead,
) -> swoole_rs_server::SendData<'a> {
    use swoole_rs_server::SendData;
    if package.as_ptr().is_null() {
        SendData::empty(info)
    } else {
        SendData::new(info, package.bytes())
    }
}

/// A session out of the reactor whose connection no one is closing: `pause` took its
/// socket out to stop reading.
pub(crate) fn paused(conn: swoole_rs_server::Connection<'_>) -> bool {
    conn.is_active()
        && conn.closed().load(Ordering::Relaxed) == 0
        && !(conn.closing()
            || conn.close_actively()
            || conn.close_queued()
            || conn.close_reset()
            || conn.close_force()
            || conn.peer_closed())
}
