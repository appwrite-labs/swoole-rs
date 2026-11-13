//! `swoole_client_select()` (and its alias `swoole_select()`): `poll(2)` over
//! arrays of clients, sockets and streams, which it narrows to the ready ones.
use crate::{
    native::ErrorKind,
    net::{CONNECT_TIMEOUT, bailed, errno, warn, warn_sys},
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

/// `php_swoole_array_length_safe`.
fn length(array: Option<&Owned>) -> usize {
    array
        .and_then(|array| array.array())
        .map_or(0, |array| array.count() as usize)
}

/// `php_swoole_convert_to_fd`, which may warn.
fn fd_of(element: &Value) -> Option<c_int> {
    bailed(swoole_rs_phpruntime::convert_to_fd(element))
}

/// `client_poll_get`: the slot polling `fd` among the first `count`.
fn slot(fds: &[pollfd], count: usize, fd: c_int) -> Option<usize> {
    fds.iter().take(count).position(|entry| entry.fd == fd)
}

/// `client_poll_add`: poll each element's fd for `event`. Every fd gets its
/// own slot for reading; writing and errors join a slot already polling it.
fn add(array: &Array, fds: &mut Vec<pollfd>, count: usize, event: c_short) -> Option<()> {
    for (_, element) in array.entries() {
        let fd = fd_of(element)?;
        if fd < 0 {
            continue;
        }
        let index = (event != POLLIN).then(|| slot(fds, count, fd)).flatten();
        match index.and_then(|index| fds.get_mut(index)) {
            Some(entry) => {
                entry.fd = fd;
                entry.events |= event;
            }
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
fn keep_ready(array: &Array, fds: &[pollfd], count: usize, revent: c_short) -> Option<OwnedArray> {
    let mut ready = OwnedArray::default();
    let mut entries = array.entries();
    while let Some((key, index, element)) = entries.next_stored() {
        let fd = fd_of(element)?;
        if fd < 0 {
            continue;
        }
        let Some(polled) = slot(fds, count, fd) else {
            warn(ErrorKind::Fatal, &format!("bad fd[{fd}]"));
            continue;
        };
        if fds
            .get(polled)
            .is_some_and(|entry| entry.revents & revent != 0)
        {
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
fn select(call: &Call) {
    let parsed = call.arity(3, 4).and_then(|()| {
        Ok((
            length(call.array_or_null(1)?.as_ref()),
            length(call.array_or_null(2)?.as_ref()),
            length(call.array_or_null(3)?.as_ref()),
            call.double(4, CONNECT_TIMEOUT)?,
        ))
    });
    let Ok((read, write, except, timeout)) = parsed else {
        return;
    };
    let count = read.max(write).max(except);
    let mut fds = Vec::with_capacity(count);
    for (n, event) in SETS {
        if let Some(set) = set(call, n)
            && let Some(array) = set.array()
            && add(array, &mut fds, count, event).is_none()
        {
            return call.set_bool(false);
        }
    }
    if fds.is_empty() {
        warn(ErrorKind::Fatal, "no resource arrays were passed to select");
        return call.set_bool(false);
    }
    // `poll` sees the first `count` slots, zeroed past what was added.
    fds.resize(
        fds.len().max(count),
        pollfd {
            fd: 0,
            events: 0,
            revents: 0,
        },
    );
    let wait = (timeout * 1000.0).c_cast::<c_int>();
    let ready = loop {
        let polled = fds.get_mut(..count).unwrap_or_default();
        let ready = swoole_rs_corebase::sys::poll(polled, wait);
        if !(ready < 0 && errno() == libc::EINTR) {
            break ready;
        }
    };
    if ready == -1 {
        warn_sys("unable to poll()", errno());
        return call.set_bool(false);
    }
    for (n, event) in SETS {
        let Some(set) = set(call, n) else {
            continue;
        };
        let Some(ready) = set
            .array()
            .and_then(|array| keep_ready(array, &fds, count, event))
        else {
            return call.set_bool(false);
        };
        // SAFETY: nothing is borrowed from argument `n`: `set` holds its own
        // reference, and every later argument is read afresh.
        unsafe { call.assign(n, ready.into_owned()) };
    }
    call.set_long(i64::from(ready));
}

swoole_rs_zend::handlers! {
    zif_swoole_client_select => select;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(fd: c_int) -> pollfd {
        pollfd {
            fd,
            events: 0,
            revents: 0,
        }
    }

    #[test]
    fn slots_are_searched_within_the_polled_count() {
        let fds = [entry(3), entry(4), entry(5)];
        assert_eq!(slot(&fds, 3, 4), Some(1), "found");
        assert_eq!(slot(&fds, 2, 5), None, "past the count");
    }
}
