//! Descriptor admission over actual Zend values and SDK stream/socket leaves.
use super::native;
use std::ffi::CStr;
use swoole_rs_zend::{Value, ZString, bailout::Bailout, message, sys as z};

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

/// `php_swoole_convert_to_fd(value)`: an fd, or -1 after a warning.
pub(super) fn convert(value: &Value) -> Result<i32, Bailout> {
    match value.kind() {
        z::IS_RESOURCE => {
            if let Some(fd) = value.stream_fd()? {
                return Ok(fd);
            }
            warning(c"fd argument must be either valid PHP stream or valid PHP socket resource")?;
            Ok(-1)
        }
        z::IS_LONG => {
            let fd = native_fd(value.long());
            if fd < 0 {
                warning(message!("invalid file descriptor#{fd} passed").as_cstr())?;
                return Ok(-1);
            }
            Ok(fd)
        }
        z::IS_OBJECT => {
            let families = swoole_rs_phpswoole::families();
            let owners = [
                (
                    families.map(|families| (families.socket_class)()),
                    b"fd".as_slice(),
                ),
                (families.map(|families| (families.client_class)()), b"sock"),
                (Some(swoole_rs_phpprocess::process_class()), b"pipe"),
            ];
            for (class, property) in owners {
                if class.is_some_and(|class| value.instance_of(class)) {
                    let fd = value.property_long(&ZString::new(property))?;
                    return Ok(fd.map_or(-1, native_fd));
                }
            }
            if let Some((fd, closed)) = native::php_socket_fd(value) {
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
