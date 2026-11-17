//! `stream_select` in a coroutine (`HOOK_STREAM_FUNCTION`): the streams'
//! fds are polled by `System::socket_poll`, which yields; streams with
//! buffered reads answer at once, as PHP's own `stream_select` does.
use crate::socket::{Report, convert_to_fd, coroutine_check, report};
use std::collections::HashMap;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::reactor::Events;
use swoole_rs_hooks::poll::socket_poll_rows as poll;
use swoole_rs_zend::{
    ArrayMut, Call, CallFrame, E_WARNING, Owned, Value, ZStr, ZString,
    bailout::Bailout,
    sys::{zend_long, zend_ulong},
};

unsafe extern "C" {
    /// Whether `value` holds a stream resource (`php_stream_from_zval_no_verify`)
    /// whose read buffer has bytes not yet read; false for any other value or
    /// resource. It reads the zval, the resource's type and the stream's two
    /// positions, and runs no PHP code.
    safe fn swr_zend_stream_zval_buffered(value: &Value) -> bool;
    /// `stream_select`'s `ZEND_PARSE_PARAMETERS` block over the call's frame
    /// (Zend reports a failure): 1 if the arguments parsed, each output then
    /// written, the arrays as null (`None`) or the array zval a by-reference
    /// argument refers to, separated, which the frame's reference keeps for the
    /// call. A coercion's deprecation runs the error handler, which may bail out.
    safe fn swr_zend_runtime_select_args<'a>(
        frame: CallFrame<'a>,
        read: &mut Option<&'a Value>,
        write: &mut Option<&'a Value>,
        except: &mut Option<&'a Value>,
        seconds: &mut zend_long,
        seconds_null: &mut bool,
        micros: &mut zend_long,
        micros_null: &mut bool,
    ) -> std::ffi::c_int;
}

/// The event of each array: read, write, except.
const EVENTS: [Events; 3] = [Events::READ, Events::WRITE, Events::ERROR];

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

/// `$read`, `$write` and `$except` as parsing left them: none, or the zval a
/// by-reference argument refers to. Each array is edited lent out of its zval
/// ([`Value::edit_array`]), as the same array may be passed twice by reference.
struct Arrays([Option<u32>; 3]);

impl Arrays {
    /// `edit` of array `which`; None when none was passed or its variable no
    /// longer holds an array.
    fn edit<R>(
        &self,
        call: &mut Call,
        which: usize,
        edit: impl FnOnce(&mut ArrayMut) -> R,
    ) -> Option<R> {
        let argument = self.0.get(which).copied().flatten()?;
        call.edit_arg_array(argument, edit)
    }
}

/// `stream_array_to_fd_set`: each stream's fd polled for `event`; the first
/// entry of an fd is where its stream goes back. `array` is a copy: a
/// warning's handler may assign the by-reference argument while the walk
/// goes on, and the copy keeps the table it walks alive.
fn to_fd_set(
    array: &Owned,
    event: Events,
    rows: &mut Vec<(i32, Events)>,
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
            if swr_zend_stream_zval_buffered(&value) {
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

swoole_rs_zend::handlers! {
    zif_swoole_stream_select => stream_select;
}

/// `stream_select(?array &$read, ?array &$write, ?array &$except, ?int
/// $seconds, ?int $microseconds = null): int|false`.
fn stream_select(call: &mut Call) {
    swoole_rs_zend::bailout::defer(select(call));
}

fn select(call: &mut Call) -> Result<(), Bailout> {
    coroutine_check()?;
    let (mut read, mut write, mut except) = (None, None, None);
    let (mut seconds, mut micros): (zend_long, zend_long) = (0, 0);
    let (mut seconds_null, mut micros_null) = (false, true);
    let mut parsed = 0;
    swoole_rs_zend::contain_with(|| {
        parsed = swr_zend_runtime_select_args(
            call.call_frame(),
            &mut read,
            &mut write,
            &mut except,
            &mut seconds,
            &mut seconds_null,
            &mut micros,
            &mut micros_null,
        );
    })?;
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
        {
            timeout = seconds.c_cast::<f64>() + micros.c_cast::<f64>() / 1_000_000.0;
        }
    }

    let arrays = Arrays([read.map(|_| 1), write.map(|_| 2), except.map(|_| 3)]);
    let mut rows = Vec::new();
    let mut found = HashMap::new();
    for (which, event) in EVENTS.into_iter().enumerate() {
        if let Some(array) = arrays.edit(call, which, |array| array.copy()) {
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
    let ready = arrays.edit(call, 0, emulate_read).unwrap_or(0);
    // `$read` holds only its ready streams then; the others are emptied.
    for which in usize::from(ready > 0)..EVENTS.len() {
        arrays.edit(call, which, |array| {
            if let Some(array) = array.separate() {
                array.clear();
            }
        });
    }
    if ready > 0 {
        call.set_long(zend_long::try_from(ready).unwrap_or(zend_long::MAX));
        return Ok(());
    }

    // a timeout or a failure to add a socket
    let Ok(polled) = poll(&rows, timeout) else {
        call.set_long(0);
        return Ok(());
    };
    let mut ready = 0;
    for (fd, revents) in polled {
        if revents.is_empty() {
            continue;
        }
        let Some(entry) = found.get(&fd) else {
            continue;
        };
        for (which, event) in EVENTS.into_iter().enumerate() {
            if revents.intersects(event) {
                arrays.edit(call, which, |array| entry.add_to(array));
            }
        }
        ready += 1;
    }
    call.set_long(ready);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{EVENTS, Events};

    #[test]
    fn events_are_swooles() {
        assert_eq!(EVENTS.map(Events::bits), [512, 1024, 2048]);
    }
}
