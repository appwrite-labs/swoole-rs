//! `stream_select` in a coroutine (`HOOK_STREAM_FUNCTION`): the streams'
//! fds are polled by `System::socket_poll`, which yields; streams with
//! buffered reads answer at once, as PHP's own `stream_select` does.
use crate::socket::{Report, convert_to_fd, coroutine_check, poll, report};
use std::{collections::HashMap, ptr};
use swoole_rs_zend::{
    Call, E_WARNING,
    bailout::Bailout,
    sys::{
        _zend_new_array, IS_ARRAY_EX, swr_zend_hash_foreach_key, swr_zend_runtime_select_args,
        swr_zend_stream_from_zval, swr_zend_string_copy, swr_zend_string_release,
        swr_zval_try_addref, zend_argument_value_error, zend_array, zend_array_destroy,
        zend_hash_clean, zend_hash_index_update, zend_hash_update, zend_long, zend_string,
        zend_ulong, zend_value, zval, zval_add_ref, zval_ptr_dtor,
    },
    type_of,
    value::{set, table},
};

/// `SW_EVENT_READ`, `SW_EVENT_WRITE`, `SW_EVENT_ERROR`.
const READ: i32 = 1 << 9;
const WRITE: i32 = 1 << 10;
const ERROR: i32 = 1 << 11;

/// `zend::KeyValue`: where a stream sat in its array, and a reference to it.
struct Entry {
    index: zend_ulong,
    key: *mut zend_string,
    value: zval,
}

impl Entry {
    /// # Safety
    /// `value` is a live, dereferenced zval; `key` is null or live.
    unsafe fn new(index: zend_ulong, key: *mut zend_string, value: *mut zval) -> Self {
        let key = if key.is_null() {
            key
        } else {
            // SAFETY: per the caller.
            unsafe { swr_zend_string_copy(key) }
        };
        // SAFETY: per the caller.
        let mut value = unsafe { *value };
        // SAFETY: our copy takes its own reference.
        unsafe { swr_zval_try_addref(&raw mut value) };
        Self { index, key, value }
    }

    /// Put the stream back in `array` under its key.
    fn add_to(&mut self, array: *mut zval) {
        // SAFETY: a live array zval.
        let table = unsafe { table(array) };
        let slot = if self.key.is_null() {
            // SAFETY: a live table; Zend copies the zval.
            unsafe { zend_hash_index_update(table, self.index, &raw mut self.value) }
        } else {
            // SAFETY: as above.
            unsafe { zend_hash_update(table, self.key, &raw mut self.value) }
        };
        // SAFETY: the table's new slot takes its own reference.
        unsafe { swr_zval_try_addref(slot) };
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        if !self.key.is_null() {
            // SAFETY: our copy.
            unsafe { swr_zend_string_release(self.key) };
        }
        // SAFETY: our reference.
        unsafe { zval_ptr_dtor(&raw mut self.value) };
    }
}

/// Each `(index, key, value)` of a table, values dereferenced.
fn entries(table: *mut zend_array) -> Vec<(zend_ulong, *mut zend_string, *mut zval)> {
    unsafe extern "C" fn each(
        index: zend_ulong,
        key: *mut zend_string,
        value: *mut zval,
        context: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: the vector below.
        let rows =
            unsafe { &mut *context.cast::<Vec<(zend_ulong, *mut zend_string, *mut zval)>>() };
        rows.push((index, key, deref(value)));
        true
    }
    let mut rows: Vec<(zend_ulong, *mut zend_string, *mut zval)> = Vec::new();
    // SAFETY: a live table; `each` gets the vector, live for the call.
    unsafe { swr_zend_hash_foreach_key(table, Some(each), (&raw mut rows).cast()) };
    rows
}

/// `ZVAL_DEREF`.
fn deref(value: *mut zval) -> *mut zval {
    // SAFETY: a live zval.
    if unsafe { type_of(value) } != swoole_rs_zend::sys::IS_REFERENCE {
        return value;
    }
    // SAFETY: a live zval.
    let inner = unsafe { (*value).value };
    // SAFETY: a reference zval holds `ref_`.
    let reference = unsafe { inner.ref_ };
    // SAFETY: a live reference.
    unsafe { &raw mut (*reference).val }
}

/// `stream_array_to_fd_set`: each stream's fd polled for `event`; the first
/// entry of an fd is where its stream goes back.
fn to_fd_set(
    array: *mut zval,
    event: i32,
    rows: &mut Vec<(i32, i32)>,
    found: &mut HashMap<i32, Entry>,
) -> Result<(), Bailout> {
    // SAFETY: an array zval.
    for (index, key, value) in entries(unsafe { table(array) }) {
        // SAFETY: a live element.
        let fd = unsafe { convert_to_fd(value) }?;
        if fd < 0 {
            continue;
        }
        // SAFETY: a live element and key.
        found
            .entry(fd)
            .or_insert_with(|| unsafe { Entry::new(index, key, value) });
        rows.push((fd, event));
    }
    Ok(())
}

/// `stream_array_emulate_read_fd_set`: keep only the streams with buffered
/// data, if any; how many.
fn emulate_read(array: *mut zval) -> i64 {
    // SAFETY: an array zval.
    let old = unsafe { table(array) };
    // SAFETY: a live table.
    let size = unsafe { (*old).nNumOfElements };
    // SAFETY: a fresh table.
    let ready = unsafe { _zend_new_array(size) };
    let mut count = 0;
    for (index, key, value) in entries(old) {
        // SAFETY: a live element.
        let stream = unsafe { swr_zend_stream_from_zval(value) };
        // SAFETY: null or a live stream.
        let Some(stream) = (unsafe { stream.as_ref() }) else {
            continue;
        };
        if stream.writepos - stream.readpos <= 0 {
            continue;
        }
        let slot = if key.is_null() {
            // SAFETY: the new table copies the zval.
            unsafe { zend_hash_index_update(ready, index, value) }
        } else {
            // SAFETY: as above.
            unsafe { zend_hash_update(ready, key, value) }
        };
        // SAFETY: the slot takes its own reference.
        unsafe { zval_add_ref(slot) };
        count += 1;
    }
    if count > 0 {
        // SAFETY: the old table goes; the array holds the new one.
        unsafe { zend_array_destroy(old) };
        // SAFETY: as above.
        unsafe { set(array, zend_value { arr: ready }, IS_ARRAY_EX) };
    } else {
        // SAFETY: unused.
        unsafe { zend_array_destroy(ready) };
    }
    count
}

fn clean(array: *mut zval) {
    if !array.is_null() {
        // SAFETY: an array zval.
        let table = unsafe { table(array) };
        // SAFETY: a live table.
        unsafe { zend_hash_clean(table) };
    }
}

/// `stream_select(?array &$read, ?array &$write, ?array &$except, ?int
/// $seconds, ?int $microseconds = null): int|false`.
/// # Safety
/// Called by Zend with a live frame and return slot.
pub(crate) unsafe extern "C" fn stream_select(
    frame: *mut swoole_rs_zend::sys::zend_execute_data,
    value: *mut zval,
) {
    // SAFETY: per the caller.
    let result = select(&unsafe { Call::new(frame, value) }, frame);
    swoole_rs_zend::bailout::raise(result);
}

fn select(call: &Call, frame: *mut swoole_rs_zend::sys::zend_execute_data) -> Result<(), Bailout> {
    coroutine_check()?;
    let (mut read, mut write, mut except) = (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
    let (mut seconds, mut micros): (zend_long, zend_long) = (0, 0);
    let (mut seconds_null, mut micros_null) = (false, true);
    // SAFETY: the live frame; each output is a writable local.
    let parsed = unsafe {
        swr_zend_runtime_select_args(
            frame,
            &raw mut read,
            &raw mut write,
            &raw mut except,
            &raw mut seconds,
            &raw mut seconds_null,
            &raw mut micros,
            &raw mut micros_null,
        )
    };
    if parsed == 0 {
        call.set_bool(false);
        return Ok(());
    }
    if seconds_null && !micros_null && micros != 0 {
        // SAFETY: a handler's context; the message has no conversions.
        unsafe {
            zend_argument_value_error(
                5,
                c"must be null when argument #4 ($seconds) is null".as_ptr(),
            );
        }
        return Ok(());
    }
    let mut timeout = -1.0;
    if !seconds_null {
        let warning = if seconds < 0 {
            Some(c"The seconds parameter must be greater than 0")
        } else if micros < 0 {
            Some(c"The microseconds parameter must be greater than 0")
        } else {
            None
        };
        if let Some(warning) = warning {
            report(Report::Docref, E_WARNING, warning)?;
            call.set_bool(false);
            return Ok(());
        }
        #[expect(clippy::cast_precision_loss, reason = "upstream's (double) casts")]
        {
            timeout = seconds as f64 + micros as f64 / 1_000_000.0;
        }
    }

    let mut rows = Vec::new();
    let mut found = HashMap::new();
    for (array, event) in [(read, READ), (write, WRITE), (except, ERROR)] {
        if !array.is_null() {
            to_fd_set(array, event, &mut rows, &mut found)?;
        }
    }
    if found.is_empty() {
        report(Report::Docref, E_WARNING, c"No stream arrays were passed")?;
        call.set_bool(false);
        return Ok(());
    }

    // slight hack to support buffered data: if streams in the read array
    // have buffered reads, answer with only those
    if !read.is_null() {
        let ready = emulate_read(read);
        if ready > 0 {
            clean(write);
            clean(except);
            call.set_long(ready);
            return Ok(());
        }
    }
    clean(read);
    clean(write);
    clean(except);

    // a timeout or a failure to add a socket
    let Some(polled) = poll(&rows, timeout) else {
        call.set_long(0);
        return Ok(());
    };
    let mut ready = 0;
    for (fd, revents) in polled {
        if revents <= 0 {
            continue;
        }
        let Some(entry) = found.get_mut(&fd) else {
            continue;
        };
        for (array, event) in [(read, READ), (write, WRITE), (except, ERROR)] {
            if revents & event != 0 && !array.is_null() {
                entry.add_to(array);
            }
        }
        ready += 1;
    }
    call.set_long(ready);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ERROR, READ, WRITE};

    #[test]
    fn events_are_swooles() {
        assert_eq!((READ, WRITE, ERROR), (512, 1024, 2048));
    }
}
