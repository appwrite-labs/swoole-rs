//! Object and imported-resource custody for Coroutine Socket.
use super::{Data, SocketObject};
use crate::native::{self, Co};
use std::ptr;
use swoole_rs_zend::{bailout::Bailout, sys as z, type_of};

fn data(object: *mut z::zend_object) -> *mut Data {
    object.wrapping_byte_sub(SocketObject::OFFSET).cast()
}

/// Free the length callback, then the owned core socket after releasing any
/// imported stream reference. Upstream's free leaves the callback to the exit.
/// Do not call close: shutdown can otherwise resume other coroutines.
/// # Safety
/// Zend is freeing this live socket object, and no method remains suspended on it.
pub(super) unsafe fn release(object: *mut z::zend_object) -> Result<(), Bailout> {
    let state = data(object);
    // SAFETY: copy the core pointer of this initialized object.
    let socket = unsafe { (*state).socket };
    if socket.is_null() {
        return Ok(());
    }
    // SAFETY: the object's live core socket.
    let co = unsafe { Co::new(socket) };
    let freed = free_length_callable(co);
    // SAFETY: the embedded initialized zval outlives the native destructor leaf.
    let stream = unsafe { &raw mut (*state).stream };
    // SAFETY: this embedded slot is an initialized zval.
    if unsafe { type_of(stream) } != z::IS_UNDEF {
        // The still-owned core socket relinquishes the fd before the stream closes it.
        co.move_fd();
        // SAFETY: the object owns this stream reference; the leaf contains PHP destructor bailouts.
        if unsafe { z::swr_zend_try_zval_dtor(stream) } != 0 {
            return Err(Bailout);
        }
    }
    // SAFETY: the object's core ownership is transferred exactly once to its native destructor.
    freed.and(unsafe { native::free_socket(socket) })
}

/// `sw_callable_free(protocol.private_data_1)`: upstream's ordering publishes null
/// only after the callback's destruction returns, which frees it even when it bails out.
fn free_length_callable(socket: Co) -> Result<(), Bailout> {
    // Copy the owned callback pointer without borrowing the protocol across PHP.
    let callable = socket.record().protocol.private_data_1;
    if callable.is_null() {
        return Ok(());
    }
    let freed = native::callable_free(callable);
    socket.record().protocol.private_data_1 = ptr::null_mut();
    freed
}

/// Free the length callback, then close an imported stream or the owned core socket.
/// # Safety
/// This is an open, owned socket object held live by the current PHP method.
pub(super) unsafe fn close(object: *mut z::zend_object) -> Result<(), Bailout> {
    let state = data(object);
    // SAFETY: copy the live core pointer before effects can reenter PHP.
    let socket = unsafe { (*state).socket };
    // SAFETY: the open core socket of the live object.
    let socket = unsafe { Co::new(socket) };
    free_length_callable(socket)?;
    // SAFETY: the live object's embedded stream slot stays allocated through the close leaf.
    let stream_value = unsafe { &raw mut (*state).stream };
    // SAFETY: the embedded zval was initialized when this object was created.
    if unsafe { type_of(stream_value) } == z::IS_UNDEF {
        return socket.close_trapped().map(drop);
    }
    // SAFETY: this is the initialized imported stream zval used by the original no-verify lookup.
    let stream = unsafe { z::swr_zend_stream_from_zval(stream_value) };
    if !stream.is_null() {
        let mut flags = z::PHP_STREAM_FREE_KEEP_RSRC | z::PHP_STREAM_FREE_CLOSE;
        // SAFETY: read the real PHP stream bitfield before calling its close operation.
        if unsafe { (*stream).is_persistent() } != 0 {
            flags |= z::PHP_STREAM_FREE_CLOSE_PERSISTENT;
        }
        let flags = i32::try_from(flags).unwrap_or_else(|_| std::process::abort());
        // SAFETY: PHP owns this stream; retain its resource slot while closing it inside zend_try.
        if unsafe { z::swr_zend_try_stream_free(stream, flags) } != 0 {
            return Err(Bailout);
        }
    }
    // SAFETY: the original close discards this now-invalid resource slot without another dtor.
    unsafe {
        (*stream_value).u1 = z::_zval_struct__bindgen_ty_1 {
            type_info: z::IS_UNDEF,
        }
    };
    // The imported stream closed the fd; the core socket must no longer own it.
    socket.move_fd();
    Ok(())
}

/// One primitive property write; no Rust-owned data is jumped over by PHP magic methods.
/// # Safety
/// The class, object and property name are live and belong to this PHP thread.
pub(super) unsafe fn update_long(
    scope: *mut z::zend_class_entry,
    object: *mut z::zend_object,
    name: &std::ffi::CStr,
    number: i64,
) -> Result<(), Bailout> {
    let mut value = swoole_rs_zend::long_zval(number);
    // SAFETY: the initialized scalar and borrowed name remain live through the trapped update.
    if unsafe {
        z::swr_zend_try_facade_update(
            scope,
            object,
            name.as_ptr(),
            name.count_bytes(),
            &raw mut value,
        )
    } != 0
    {
        return Err(Bailout);
    }
    Ok(())
}

/// Construct the PHP object over an already allocated core socket.
/// # Safety
/// The core socket is live; ownership transfers to the new object.
pub(super) unsafe fn make_object(socket: native::Socket) -> Result<*mut z::zend_object, Bailout> {
    // SAFETY: the registered class's data layout is zero-initializable.
    let object = unsafe { SocketObject::try_create(&super::SOCKET, super::SOCKET.entry()) }?;
    let state = data(object);
    // SAFETY: the new unpublished object now owns this core socket.
    unsafe { (*state).socket = socket };
    // SAFETY: the new object now holds its core socket and initialized data.
    unsafe { super::init(object) }?;
    Ok(object)
}

unsafe fn checked_object(socket: native::Socket) -> Result<*mut z::zend_object, Bailout> {
    // SAFETY: a freshly constructed core socket supports the scalar descriptor query.
    if unsafe { Co::new(socket) }.fd() < 0 {
        let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        crate::net::warn_sys("new Socket() failed", error);
        if swoole_rs_zend::bailout::pending() {
            return Err(Bailout);
        }
        // SAFETY: no object took ownership of this failed core socket.
        unsafe { native::free_socket(socket) }?;
        return Ok(ptr::null_mut());
    }
    // SAFETY: the validated core socket transfers ownership to the new PHP object.
    unsafe { make_object(socket) }
}

/// A new socket object of a native Swoole socket kind.
pub(super) fn create_kind(kind: i32) -> Result<*mut z::zend_object, Bailout> {
    let socket = native::make(-1, 0, kind, 0, false)?;
    // SAFETY: the returned core socket is uniquely owned and not yet published.
    unsafe { checked_object(socket) }
}

/// # Safety
/// The caller transfers this descriptor, classified by its native Swoole socket kind.
pub(crate) unsafe fn from_fd(fd: i32, kind: i32) -> Result<*mut z::zend_object, Bailout> {
    let socket = native::make(fd, 0, kind, 0, false)?;
    // SAFETY: the returned core socket is uniquely owned and not yet published.
    unsafe { checked_object(socket) }
}

/// An object over a duplicate of `fd`, which stays borrowed.
pub(super) fn duplicate(fd: i32, kind: i32) -> Result<*mut z::zend_object, Bailout> {
    // SAFETY: the existing native reactor hook runs entirely below the C bailout trap.
    if unsafe { z::swr_zend_try_hook(Some(native::swoole_rs_check_reactor)) } != 0 {
        return Err(Bailout);
    }
    // SAFETY: dup borrows the source descriptor and creates an independent owned descriptor.
    let duplicate = unsafe { libc::dup(fd) };
    if duplicate < 0 {
        let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        crate::net::warn_sys(&format!("dup({fd}) failed"), error);
        return if swoole_rs_zend::bailout::pending() {
            Err(Bailout)
        } else {
            Ok(ptr::null_mut())
        };
    }
    // SAFETY: the newly duplicated descriptor transfers to the core constructor and PHP object.
    unsafe { from_fd(duplicate, kind) }
}

/// Import retains the stream reference and its original descriptor mode after object creation.
/// # Safety
/// The descriptor/classification and initialized stream zval remain live through construction.
pub(crate) unsafe fn import_stream(
    fd: i32,
    kind: i32,
    nonblock: i32,
    stream: *mut z::zval,
) -> Result<*mut z::zend_object, Bailout> {
    // SAFETY: importing transfers descriptor custody through the same native constructor path.
    let object = unsafe { from_fd(fd, kind) }?;
    if object.is_null() {
        return Ok(object);
    }
    let state = data(object);
    // SAFETY: the imported resource is borrowed until its new reference is retained below.
    let resource = unsafe { stream.read() };
    // SAFETY: the new object's embedded stream slot was undefined and takes this shallow value.
    unsafe { (*state).stream = resource };
    // SAFETY: this initialized resource now holds the new object's independent reference.
    let held_stream = unsafe { &raw mut (*state).stream };
    // SAFETY: retaining a live resource runs no PHP callbacks.
    unsafe { z::swr_zval_try_addref(held_stream) };
    // SAFETY: copy the live core socket pointer without retaining a data borrow.
    let socket = unsafe { (*state).socket };
    // SAFETY: the new object's live core socket.
    let socket = unsafe { Co::new(socket) };
    // Preserve the original one-bit nonblock assignment, rather than normalize other bits.
    if let Some(net) = socket.net() {
        net.edit(|record| record.output.nonblock = u8::from(nonblock & 1 != 0));
    }
    Ok(object)
}

/// The PHP constructor's complete core allocation/error path.
/// # Safety
/// Native domain/type/protocol arguments follow the original C-int narrowing contract.
#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream narrows each zend_long to the C int constructor on Linux/macOS"
)]
pub(super) unsafe fn construct_core(
    domain: i64,
    kind: i64,
    protocol: i64,
) -> Result<(native::Socket, i32), Bailout> {
    // SAFETY: the existing reactor hook runs below the Zend C bailout trap.
    if unsafe { z::swr_zend_try_hook(Some(native::swoole_rs_check_reactor)) } != 0 {
        return Err(Bailout);
    }
    let socket = native::make(-1, domain as i32, kind as i32, protocol as i32, true)?;
    // SAFETY: the new socket; copy the descriptor before destruction can change errno.
    if unsafe { Co::new(socket) }.fd() >= 0 {
        return Ok((socket, 0));
    }
    let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    // SAFETY: no object has taken this failed allocation's ownership.
    unsafe { native::free_socket(socket) }?;
    Ok((ptr::null_mut(), error))
}

/// Error properties share the same scope/long/string ordering as the native SDK helper.
/// # Safety
/// The scope/object and borrowed NUL-terminated message remain live through the updates.
pub(super) unsafe fn error_properties(
    scope: *mut z::zend_class_entry,
    object: *mut z::zend_object,
    code: i32,
    message: &std::ffi::CStr,
) -> Result<(), Bailout> {
    // SAFETY: the supplied live scope and object own this property.
    unsafe { update_long(scope, object, c"errCode", i64::from(code)) }?;
    // Released on drop; it holds a string, whose release runs no PHP.
    let value = swoole_rs_zend::Owned::default();
    // SAFETY: copy the borrowed diagnostic bytes into a new owned Zend string inside the C trap.
    if unsafe {
        z::swr_zend_try_facade_value_string(value.as_ptr(), message.as_ptr(), message.count_bytes())
    } != 0
    {
        return Err(Bailout);
    }
    // SAFETY: the owned string and literal property name remain live through a trapped update.
    let bailed = unsafe {
        z::swr_zend_try_facade_update(scope, object, c"errMsg".as_ptr(), 6, value.as_ptr())
    };
    if bailed == 0 { Ok(()) } else { Err(Bailout) }
}
