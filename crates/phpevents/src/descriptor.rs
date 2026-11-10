//! Descriptor admission over actual Zend values and SDK stream/socket leaves.
use super::{Message, native};
use std::{ffi::CStr, ptr};
use swoole_rs_zend::{bailout::Bailout, sys as z, type_of, value};

unsafe extern "C" {
    static mut swoole_process_ce: *mut z::zend_class_entry;
}

fn warning(message: &CStr) -> Result<(), Bailout> {
    native::fatal_warning(message)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the native descriptor API narrows zend_long to int before its sign check"
)]
fn native_fd(value: z::zend_long) -> i32 {
    value as i32
}

/// # Safety
/// `value` is a live borrowed zval; no Rust reference spans a magic-method call.
pub(super) unsafe fn convert(value: *mut z::zval) -> Result<i32, Bailout> {
    // SAFETY: the caller lends a live initialized zval.
    match unsafe { type_of(value) } {
        z::IS_RESOURCE => {
            // SAFETY: no-verify lookup only inspects the borrowed resource tag.
            let stream = unsafe { z::swr_zend_stream_from_zval(value) };
            if !stream.is_null() {
                let mut fd = -1;
                // SAFETY: the live stream is borrowed; any wrapper bailout is contained by the C shim.
                let status = unsafe { z::swr_zend_stream_select_fd(stream, &raw mut fd) };
                if status == 2 {
                    return Err(Bailout);
                }
                if status == 0 && fd >= 0 {
                    return Ok(fd);
                }
            }
            warning(c"fd argument must be either valid PHP stream or valid PHP socket resource")?;
            Ok(-1)
        }
        z::IS_LONG => {
            // SAFETY: the tag above proves the integer union member is active.
            let fd = native_fd(unsafe { value::long_of(value) });
            if fd < 0 {
                let message = Message::new(format_args!("invalid file descriptor#{fd} passed"));
                // SAFETY: the message is NUL-terminated and remains live during the report.
                warning(unsafe { CStr::from_ptr(message.as_ptr()) })?;
                return Ok(-1);
            }
            Ok(fd)
        }
        z::IS_OBJECT => {
            // SAFETY: the tag above proves the object union member is active.
            let object = unsafe { value::obj_of(value) };
            // SAFETY: a live object publishes its class entry.
            let class = unsafe { (*object).ce };
            let (co_socket, client) = swoole_rs_phpswoole::families()
                .map_or((ptr::null_mut(), ptr::null_mut()), |families| {
                    ((families.socket_class)(), (families.client_class)())
                });
            // SAFETY: an immutable module class entry, initialized before user code.
            let process = unsafe { swoole_process_ce };
            let mut property = None;
            for (index, parent) in [(0_u8, co_socket), (1, client), (2, process)] {
                // SAFETY: non-null class entries are registered and immutable during the request.
                if !parent.is_null() && unsafe { z::swr_zend_instanceof(class, parent) } {
                    property = Some(index);
                    break;
                }
            }
            if let Some(index) = property {
                let name = native::property_name(index);
                let mut found = ptr::null_mut();
                // SAFETY: the shim contains magic-method bailouts; no object borrow spans that call.
                if unsafe { z::swr_zend_event_property(value, name, &raw mut found) } != 0 {
                    return Err(Bailout);
                }
                // SAFETY: a successful property lookup lends an initialized zval.
                if !found.is_null() && unsafe { type_of(found) } == z::IS_LONG {
                    // SAFETY: the tag above proves the integer member is active.
                    return Ok(native_fd(unsafe { value::long_of(found) }));
                }
                return Ok(-1);
            }
            // SAFETY: a live object zval.
            if let Some((fd, closed)) = unsafe { native::php_socket_fd(value) } {
                if closed {
                    warning(c"contains a closed socket")?;
                    return Ok(-1);
                }
                return Ok(fd);
            }
            Ok(-1)
        }
        _ => {
            warning(c"invalid file descriptor passed")?;
            Ok(-1)
        }
    }
}
