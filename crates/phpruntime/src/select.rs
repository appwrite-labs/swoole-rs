//! `stream_select` in a coroutine (`HOOK_STREAM_FUNCTION`): the streams'
//! fds are polled by `System::socket_poll`, which yields; streams with
//! buffered reads answer at once, as PHP's own `stream_select` does.
use crate::socket::{Report, convert_to_fd, coroutine_check, poll, report};
use std::{collections::HashMap, ptr};
use swoole_rs_zend::{
    ArrayMut, Call, E_WARNING, Owned, Value, ZStr, ZString,
    bailout::Bailout,
    sys::{swr_zend_runtime_select_args, swr_zend_stream_from_zval, zend_long, zend_ulong, zval},
};

/// `SW_EVENT_READ`, `SW_EVENT_WRITE`, `SW_EVENT_ERROR`, one per array.
const READ: i32 = 1 << 9;
const WRITE: i32 = 1 << 10;
const ERROR: i32 = 1 << 11;
const EVENTS: [i32; 3] = [READ, WRITE, ERROR];

/// `zend::KeyValue`: where a stream sat in its array, and a reference to it.
struct Entry {
    key: Option<ZString>,
    index: zend_ulong,
    value: Owned,
}

impl Entry {
    fn new(key: Option<&ZStr>, index: zend_ulong, value: &Value) -> Self {
        Self {
            key: key.map(ZStr::to_owned),
            index,
            value: value.copy(),
        }
    }

    /// Put the stream back in `array` under its key.
    fn add_to(&self, array: &mut ArrayMut) {
        let value = self.value.copy();
        match &self.key {
            Some(key) => array.update(key, value),
            None => array.update_index(self.index, value),
        }
    }
}

/// `$read`, `$write` and `$except` as parsing left them: null, or an array
/// zval of the frame, dereferenced and separated. One is borrowed at a time,
/// as the same array may be passed twice by reference.
struct Arrays([*mut zval; 3]);

impl Arrays {
    fn get(&mut self, which: usize) -> Option<&mut ArrayMut> {
        let array = *self.0.get(which)?;
        // SAFETY: a live array zval of the frame, which outlives the call;
        // `&mut self` keeps this the only borrow of it.
        (!array.is_null()).then(|| unsafe { ArrayMut::from_ptr(array) })
    }
}

/// `stream_array_to_fd_set`: each stream's fd polled for `event`; the first
/// entry of an fd is where its stream goes back. `array` is a copy: a
/// warning's handler may assign the by-reference argument while the walk
/// goes on, and the copy keeps the table it walks alive.
fn to_fd_set(
    array: &Owned,
    event: i32,
    rows: &mut Vec<(i32, i32)>,
    found: &mut HashMap<i32, Entry>,
) -> Result<(), Bailout> {
    let Some(array) = array.array() else {
        return Ok(());
    };
    let mut entries = array.entries();
    while let Some((key, index, value)) = entries.next_stored() {
        let value = value.dereferenced();
        let fd = convert_to_fd(&value)?;
        if fd < 0 {
            continue;
        }
        found
            .entry(fd)
            .or_insert_with(|| Entry::new(key, index, &value));
        rows.push((fd, event));
    }
    Ok(())
}

/// `stream_array_emulate_read_fd_set`: keep only the streams with buffered
/// data, if any; how many.
fn emulate_read(array: &mut ArrayMut) -> usize {
    let mut ready = Vec::new();
    if let Some(table) = array.array() {
        let mut entries = table.entries();
        while let Some((key, index, value)) = entries.next_stored() {
            let value = value.dereferenced();
            // SAFETY: a live element.
            let stream = unsafe { swr_zend_stream_from_zval(value.as_ptr()) };
            // SAFETY: null or a live stream.
            if unsafe { stream.as_ref() }.is_some_and(|stream| stream.writepos > stream.readpos) {
                ready.push(Entry::new(key, index, &value));
            }
        }
    }
    if !ready.is_empty() {
        if let Some(array) = array.separate() {
            array.clear();
        }
        for entry in &ready {
            entry.add_to(array);
        }
    }
    ready.len()
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
        call.argument_value_error(5, c"must be null when argument #4 ($seconds) is null");
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

    let mut arrays = Arrays([read, write, except]);
    let mut rows = Vec::new();
    let mut found = HashMap::new();
    for (which, event) in EVENTS.into_iter().enumerate() {
        if let Some(array) = arrays.get(which).map(|array| array.copy()) {
            to_fd_set(&array, event, &mut rows, &mut found)?;
        }
    }
    if found.is_empty() {
        report(Report::Docref, E_WARNING, c"No stream arrays were passed")?;
        call.set_bool(false);
        return Ok(());
    }

    // slight hack to support buffered data: if streams in the read array
    // have buffered reads, answer with only those
    let ready = arrays.get(0).map_or(0, emulate_read);
    // `$read` holds only its ready streams then; the others are emptied.
    for which in usize::from(ready > 0)..EVENTS.len() {
        if let Some(array) = arrays.get(which).and_then(ArrayMut::separate) {
            array.clear();
        }
    }
    if ready > 0 {
        call.set_long(zend_long::try_from(ready).unwrap_or(zend_long::MAX));
        return Ok(());
    }

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
        let Some(entry) = found.get(&fd) else {
            continue;
        };
        for (which, event) in EVENTS.into_iter().enumerate() {
            if revents & event != 0
                && let Some(array) = arrays.get(which)
            {
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
