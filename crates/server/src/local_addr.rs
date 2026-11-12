//! The server's local interface addresses, numbered from 1 in `getifaddrs` order
//! across both families. A connection stores the number of its local address
//! (`Connection::local_addr_index`) and `Server::get_local_addr` reads its text.
use core::ffi::{c_char, c_int};
use std::ffi::{CStr, CString};

use swoole_rs_corebase::log;

use crate::port_config::PortConfig;
use crate::records::ConnectionRecord;
use crate::{ServerState, view};

const SW_SOCK_TCP: c_int = 1;
const SW_SOCK_TCP6: c_int = 3;

struct Entry {
    index: u16,
    v6: bool,
    /// `in_addr` or `in6_addr` bytes.
    bytes: Vec<u8>,
    text: CString,
}

/// Owned by `ServerState::local_addrs`.
pub struct LocalAddrs {
    entries: Vec<Entry>,
}

impl LocalAddrs {
    fn push(&mut self, v6: bool, bytes: &[u8]) {
        let index = u16::try_from(self.entries.len() + 1).unwrap_or(u16::MAX);
        self.entries.push(Entry {
            index,
            v6,
            bytes: bytes.to_vec(),
            text: ntop(v6, bytes),
        });
    }

    fn index(&self, v6: bool, bytes: &[u8]) -> u16 {
        self.entries
            .iter()
            .find(|entry| entry.v6 == v6 && entry.bytes == bytes)
            .map_or(0, |entry| entry.index)
    }

    fn text(&self, v6: bool, index: u16) -> Option<&CStr> {
        self.entries
            .iter()
            .find(|entry| entry.v6 == v6 && entry.index == index)
            .map(|entry| entry.text.as_c_str())
    }
}

unsafe extern "C" {
    // The `libc` crate does not bind it.
    fn inet_ntop(
        af: c_int,
        src: *const core::ffi::c_void,
        dst: *mut c_char,
        size: libc::socklen_t,
    ) -> *const c_char;
}

/// `inet_ntop`, as `network::Address::get_addr` formats.
fn ntop(v6: bool, bytes: &[u8]) -> CString {
    let family = if v6 { libc::AF_INET6 } else { libc::AF_INET };
    let mut out = [0_u8; 64];
    let len = libc::socklen_t::try_from(out.len()).unwrap_or(0);
    // SAFETY: `bytes` holds an address of `family` and `out` has `len` bytes.
    let text = unsafe { inet_ntop(family, bytes.as_ptr().cast(), out.as_mut_ptr().cast(), len) };
    if text.is_null() {
        return CString::default();
    }
    // `inet_ntop` wrote a terminated string into `out`.
    CStr::from_bytes_until_nul(&out)
        .map(CStr::to_owned)
        .unwrap_or_default()
}

/// Reads the interfaces that are up. `None` when `getifaddrs` fails, with `errno` set.
fn read() -> Option<LocalAddrs> {
    let list = swoole_rs_corebase::sys::Interfaces::new()?;
    let mut addrs = LocalAddrs {
        entries: Vec::new(),
    };
    for entry in list.iter() {
        if entry.flags() & libc::IFF_UP.cast_unsigned() == 0 {
            continue;
        }
        match entry.ip() {
            Some(std::net::IpAddr::V4(ip)) => addrs.push(false, &ip.octets()),
            Some(std::net::IpAddr::V6(ip)) => addrs.push(true, &ip.octets()),
            None => {}
        }
    }
    Some(addrs)
}

/// `Server::init_network_interface_addr_map()`, from `Server::create` before any
/// reactor thread reads the table. A `create` retried after a failure keeps the
/// table its first attempt read, which its readers may already hold.
pub(crate) fn init(state: &ServerState) -> bool {
    let Some(addrs) = read() else {
        log::sys_warning(
            c"Server::init_network_interface_addr_map",
            format_args!("getifaddrs() failed"),
        );
        return false;
    };
    // A table already there stays; this one drops.
    state.local_addrs.set(addrs).ok();
    true
}

fn table(state: &ServerState) -> Option<&LocalAddrs> {
    state.local_addrs.get()
}

/// The number of the local address `addr` (an `in_addr`'s bytes, or an `in6_addr`'s
/// when `v6`), 0 when unknown.
pub(crate) fn index(state: &ServerState, v6: bool, addr: &[u8]) -> u16 {
    table(state).map_or(0, |addrs| addrs.index(v6, addr))
}

/// The text of local address `index`; the loopback address when unknown. Valid
/// while the state lives.
fn text(state: &ServerState, v6: bool, index: u16) -> &CStr {
    let found = table(state).and_then(|addrs| addrs.text(v6, index));
    found.unwrap_or(if v6 { c"::1" } else { c"127.0.0.1" })
}

/// `Server::get_local_addr(conn)`: a TCP connection's local interface address, else
/// its port's host; empty without one.
#[must_use]
pub fn get_local_addr<'a>(serv: view::Server<'a>, conn: &ConnectionRecord) -> &'a CStr {
    if conn.socket_type == SW_SOCK_TCP || conn.socket_type == SW_SOCK_TCP6 {
        return text(
            serv.state(),
            conn.socket_type == SW_SOCK_TCP6,
            conn.local_addr_index,
        );
    }
    serv.port_of_server_fd(conn.server_fd)
        .map_or(c"", PortConfig::host_c_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no inet_ntop")]
    fn numbers_both_families_in_order() {
        let mut addrs = LocalAddrs {
            entries: Vec::new(),
        };
        addrs.push(false, &[127, 0, 0, 1]);
        let mut six = [0u8; 16];
        six[15] = 1;
        addrs.push(true, &six);
        addrs.push(false, &[10, 0, 0, 2]);
        assert_eq!(addrs.index(false, &[10, 0, 0, 2]), 3, "v4 after v6");
        assert_eq!(addrs.index(true, &six), 2, "v6");
        assert_eq!(addrs.index(false, &[1, 2, 3, 4]), 0, "unknown");
        assert_eq!(addrs.text(false, 3), Some(c"10.0.0.2"), "inet_ntop text");
        assert_eq!(addrs.text(true, 2), Some(c"::1"), "v6 text");
        assert_eq!(addrs.text(true, 3), None, "index of the other family");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no getifaddrs")]
    fn reads_the_interfaces_and_falls_back_to_loopback() {
        let state = ServerState::new();
        assert!(init(&state), "getifaddrs works here");
        let loopback = [127u8, 0, 0, 1];
        let index = super::index(&state, false, &loopback);
        assert_ne!(index, 0, "lo is up");
        assert_eq!(text(&state, false, index), c"127.0.0.1", "loopback text");
        assert_eq!(text(&state, true, 0), c"::1", "unknown v6 index");
        assert!(init(&state), "a second read replaces the first");
        let empty = ServerState::new();
        assert_eq!(super::index(&empty, false, &loopback), 0, "no table");
    }
}
