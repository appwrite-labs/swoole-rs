//! `swoole_client_select()` (and its alias `swoole_select()`): `poll(2)` over
//! arrays of clients, sockets and streams, which it narrows to the ready ones.
use crate::{
    native::{ClientCall, ErrorKind, Op},
    net::{CONNECT_TIMEOUT, bailed, errno, warn, warn_sys},
};
use libc::{POLLHUP, POLLIN, POLLOUT, c_int, c_short, nfds_t, pollfd};
use std::ptr;
use swoole_rs_zend::{
    Call, Invalid, long_zval,
    sys::{
        _zend_expected_type_Z_EXPECTED_ARRAY_OR_NULL, HashPosition, IS_ARRAY, IS_NULL,
        ZPP_ERROR_WRONG_ARG, swr_zend_array_init, swr_zval_try_addref, zend_hash_add,
        zend_hash_get_current_data_ex, zend_hash_get_current_key_ex, zend_hash_index_update,
        zend_hash_internal_pointer_reset_ex, zend_hash_move_forward_ex, zend_string, zend_ulong,
        zend_wrong_parameter_error, zval, zval_ptr_dtor,
    },
    type_of, value,
};

/// `HASH_KEY_IS_STRING`.
const KEY_IS_STRING: i64 = 1;

/// `Z_PARAM_ARRAY_EX2(dest, 1, 1, 0)`: an array, by reference, or null.
fn array_or_null(call: &Call, n: u32) -> Result<Option<*mut zval>, Invalid> {
    let arg = call.zval_deref(n);
    // SAFETY: a passed argument.
    match unsafe { type_of(arg) } {
        IS_ARRAY => Ok(Some(arg)),
        IS_NULL => Ok(None),
        _ => {
            #[expect(clippy::cast_possible_wrap, reason = "a small ZPP error code")]
            // SAFETY: as Zend's macro does, from a handler, with the argument.
            unsafe {
                zend_wrong_parameter_error(
                    ZPP_ERROR_WRONG_ARG as c_int,
                    n,
                    ptr::null_mut(),
                    _zend_expected_type_Z_EXPECTED_ARRAY_OR_NULL,
                    arg,
                );
            }
            Err(Invalid)
        }
    }
}

/// `php_swoole_array_length_safe`.
fn length(array: Option<*mut zval>) -> usize {
    // SAFETY: a live array.
    array.map_or(0, |array| unsafe { value::count(array) } as usize)
}

/// `php_swoole_convert_to_fd`, which may warn.
fn fd_of(element: *mut zval) -> Option<c_int> {
    let mut convert = ClientCall::new(ptr::null_mut(), Op::ConvertFd);
    convert.settings = element;
    let fd = bailed(convert.run())?;
    c_int::try_from(fd).ok()
}

/// The elements of `array` with their keys (string, or integer index).
fn each(array: *mut zval, mut visit: impl FnMut(*mut zend_string, zend_ulong, *mut zval) -> bool) {
    // SAFETY: a live array.
    let table = unsafe { value::table(array) };
    let mut position: HashPosition = 0;
    // SAFETY: a live table; `position` is a writable local.
    unsafe { zend_hash_internal_pointer_reset_ex(table, &raw mut position) };
    loop {
        // SAFETY: a live table and our position in it.
        let element = unsafe { zend_hash_get_current_data_ex(table, &raw mut position) };
        if element.is_null() {
            return;
        }
        let mut key: *mut zend_string = ptr::null_mut();
        let mut index: zend_ulong = 0;
        // SAFETY: as above; the outputs are writable locals.
        let kind = unsafe {
            zend_hash_get_current_key_ex(table, &raw mut key, &raw mut index, &raw const position)
        };
        // SAFETY: as above.
        unsafe { zend_hash_move_forward_ex(table, &raw mut position) };
        let key = if i64::from(kind) == KEY_IS_STRING {
            key
        } else {
            ptr::null_mut()
        };
        if !visit(key, index, element) {
            return;
        }
    }
}

/// `client_poll_get`: the slot polling `fd` among the first `count`.
fn slot(fds: &[pollfd], count: usize, fd: c_int) -> Option<usize> {
    fds.iter().take(count).position(|entry| entry.fd == fd)
}

/// `client_poll_add`: poll each element's fd for `event`. Every fd gets its
/// own slot for reading; writing and errors join a slot already polling it.
fn add(array: *mut zval, fds: &mut Vec<pollfd>, count: usize, event: c_short) -> Option<()> {
    let mut bailed_out = false;
    each(array, |_, _, element| {
        let Some(fd) = fd_of(element) else {
            bailed_out = true;
            return false;
        };
        if fd < 0 {
            return true;
        }
        match (event != POLLIN).then(|| slot(fds, count, fd)).flatten() {
            Some(index) => {
                fds[index].fd = fd;
                fds[index].events |= event;
            }
            None => fds.push(pollfd {
                fd,
                events: event,
                revents: 0,
            }),
        }
        true
    });
    (!bailed_out).then_some(())
}

/// `client_poll_wait`: keep the elements whose fd is ready for `revent`.
fn keep_ready(array: *mut zval, fds: &[pollfd], count: usize, revent: c_short) -> Option<()> {
    let mut ready = long_zval(0);
    // SAFETY: a writable local.
    unsafe { swr_zend_array_init(&raw mut ready) };
    // SAFETY: the array just made.
    let table = unsafe { value::table(&raw const ready) };
    let mut bailed_out = false;
    each(array, |key, index, element| {
        let Some(fd) = fd_of(element) else {
            bailed_out = true;
            return false;
        };
        if fd < 0 {
            return true;
        }
        let Some(polled) = slot(fds, count, fd) else {
            warn(ErrorKind::Fatal, &format!("bad fd[{fd}]"));
            return true;
        };
        if fds[polled].revents & revent == 0 {
            return true;
        }
        let added = if key.is_null() {
            // SAFETY: the new array; Zend copies the zval, whose reference we add.
            unsafe { zend_hash_index_update(table, index, element) }
        } else {
            // SAFETY: as above, under the same string key.
            unsafe { zend_hash_add(table, key, element) }
        };
        if !added.is_null() {
            // SAFETY: the element now held twice.
            unsafe { swr_zval_try_addref(added) };
        }
        true
    });
    // SAFETY: the caller's array, which the narrowed one replaces.
    unsafe { zval_ptr_dtor(array) };
    // SAFETY: as above; the new array's reference moves into it.
    unsafe { *array = ready };
    (!bailed_out).then_some(())
}

/// `swoole_client_select(?array &$read, ?array &$write, ?array &$except, float $timeout = 0.5): int|false`.
fn select(call: &Call) {
    let parsed = call.arity(3, 4).and_then(|()| {
        Ok((
            array_or_null(call, 1)?,
            array_or_null(call, 2)?,
            array_or_null(call, 3)?,
            call.double(4, CONNECT_TIMEOUT)?,
        ))
    });
    let Ok((read, write, except, timeout)) = parsed else {
        return;
    };
    let count = length(read).max(length(write)).max(length(except));
    let mut fds = Vec::with_capacity(count);
    for (array, event) in [(read, POLLIN), (write, POLLOUT), (except, POLLHUP)] {
        if let Some(array) = array.filter(|&array| length(Some(array)) > 0)
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
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's (int) (timeout * 1000)"
    )]
    let wait = (timeout * 1000.0) as c_int;
    let polled = nfds_t::try_from(count).unwrap_or(nfds_t::MAX);
    let ready = loop {
        // SAFETY: `fds` holds at least `count` entries.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), polled, wait) };
        if !(ready < 0 && errno() == libc::EINTR) {
            break ready;
        }
    };
    if ready == -1 {
        warn_sys("unable to poll()", errno());
        return call.set_bool(false);
    }
    for (array, event) in [(read, POLLIN), (write, POLLOUT), (except, POLLHUP)] {
        if let Some(array) = array.filter(|&array| length(Some(array)) > 0)
            && keep_ready(array, &fds, count, event).is_none()
        {
            return call.set_bool(false);
        }
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
