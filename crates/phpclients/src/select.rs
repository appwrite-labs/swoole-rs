//! `swoole_client_select()` (and its alias `swoole_select()`): `poll(2)` over
//! arrays of clients, sockets and streams, which it narrows to the ready ones.
use crate::{
    native::ErrorKind,
    net::{CONNECT_TIMEOUT, bailed, warn, warn_sys},
};
use libc::{POLLHUP, POLLIN, POLLOUT, c_int, c_short, pollfd};
use swoole_rs_corebase::CCast;
use swoole_rs_zend::{Array, Call, Owned, OwnedArray, Value};

/// The arrays by argument number, with the event each is polled for.
const SETS: [(u32, c_short); 3] = [(1, POLLIN), (2, POLLOUT), (3, POLLHUP)];

/// The non-empty array by-reference argument `n` refers to now: read afresh
/// each time, since two arguments may refer to the same array, which
/// [`keep_ready`] replaces. A reference of its own, since converting the
/// elements can run PHP code that assigns the argument.
fn set(call: &Call, n: u32) -> Option<Owned> {
    let value = call.value(n)?.dereferenced();
    value.array().filter(|array| array.count() > 0)?;
    Some(value.into_owned())
}

/// `php_swoole_convert_to_fd`, which may warn.
fn fd_of(element: &Value) -> Option<c_int> {
    bailed(swoole_rs_phpruntime::convert_to_fd(element))
}

/// Merge each event into the slot for its descriptor.
fn add(array: &Array, fds: &mut Vec<pollfd>, event: c_short) -> Option<()> {
    for (_, element) in array.entries() {
        let fd = fd_of(element)?;
        if fd < 0 {
            continue;
        }
        match fds.iter_mut().find(|entry| entry.fd == fd) {
            Some(entry) => entry.events |= event,
            None => fds.push(pollfd {
                fd,
                events: event,
                revents: 0,
            }),
        }
    }
    Some(())
}

/// `client_poll_wait`: the elements whose fd is ready for `revent`, under
/// their keys.
fn keep_ready(array: &Array, fds: &[pollfd], revent: c_short) -> Option<OwnedArray> {
    let mut ready = OwnedArray::default();
    let mut entries = array.entries();
    while let Some((key, index, element)) = entries.next_stored() {
        let fd = fd_of(element)?;
        if fd < 0 {
            continue;
        }
        let Some(polled) = fds.iter().find(|entry| entry.fd == fd) else {
            warn(ErrorKind::Fatal, &format!("bad fd[{fd}]"));
            continue;
        };
        if polled.revents & revent != 0 {
            // The source array's keys are unique, so none is taken yet.
            match key {
                Some(key) => ready.add_new(key, element.copy()),
                None => ready.insert_index(index, element.copy()),
            }
        }
    }
    Some(ready)
}

/// `swoole_client_select(?array &$read, ?array &$write, ?array &$except, float $timeout = 0.5): int|false`.
fn select(call: &mut Call) {
    let parsed = call.arity(3, 4).and_then(|()| {
        for (n, _) in SETS {
            call.array_or_null(n)?;
        }
        call.double(4, CONNECT_TIMEOUT)
    });
    let Ok(timeout) = parsed else {
        return;
    };
    let mut fds = Vec::new();
    for (n, event) in SETS {
        if let Some(set) = set(call, n)
            && let Some(array) = set.array()
            && add(array, &mut fds, event).is_none()
        {
            return call.set_bool(false);
        }
    }
    if fds.is_empty() {
        warn(ErrorKind::Fatal, "no resource arrays were passed to select");
        return call.set_bool(false);
    }
    let wait = (timeout * 1000.0).c_cast::<c_int>();
    let ready = loop {
        let ready = swoole_rs_corebase::sys::poll(&mut fds, wait);
        match ready {
            Ok(ready) => break ready,
            Err(error) if error.raw_os_error() == Some(libc::EINTR) => {}
            Err(error) => {
                warn_sys("unable to poll()", error.raw_os_error().unwrap_or(0));
                return call.set_bool(false);
            }
        }
    };
    for (n, event) in SETS {
        let Some(set) = set(call, n) else {
            continue;
        };
        let Some(ready) = set.array().and_then(|array| keep_ready(array, &fds, event)) else {
            return call.set_bool(false);
        };
        // `set` holds its own reference, and every later argument is read afresh.
        call.assign(n, ready.into_owned());
    }
    call.set_long(i64::from(ready));
}

swoole_rs_zend::handlers! {
    zif_swoole_client_select => select;
}
