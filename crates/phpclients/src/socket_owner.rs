//! Object and imported-resource custody for Coroutine Socket.
use super::{SOCKET, Sock, SocketObject};
use crate::native::{self, Co, NewSocket};
use swoole_rs_zend::{Owned, Value, bailout::Bailout, sys as z};

/// Free the length callback, then the owned core socket after releasing any
/// imported stream reference. Upstream's free leaves the callback to the exit.
/// Do not call close: shutdown can otherwise resume other coroutines.
/// # Safety
/// Zend is freeing this socket object, and no method remains suspended on it.
pub(super) unsafe fn release(sock: Sock) -> Result<(), Bailout> {
    let Some(co) = sock.co() else {
        return Ok(());
    };
    let freed = co.free_length_callable();
    if sock.has_stream() {
        // The still-owned core socket relinquishes the fd before the stream closes it.
        co.move_fd();
        // SAFETY: the object owns this stream reference; the leaf contains PHP destructor bailouts.
        if unsafe { z::swr_zend_try_zval_dtor(sock.stream()) } != 0 {
            return Err(Bailout);
        }
    }
    // SAFETY: the object's core ownership is transferred exactly once to its native destructor.
    freed.and(unsafe { native::free_socket(co) })
}

/// Free the length callback, then close an imported stream or the owned core socket.
pub(super) fn close(sock: Sock, socket: Co) -> Result<(), Bailout> {
    socket.free_length_callable()?;
    if !sock.has_stream() {
        return socket.close_trapped().map(drop);
    }
    let stream_value = sock.stream();
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
    // The original close discards this now-invalid resource slot without another dtor.
    sock.forget_stream();
    // The imported stream closed the fd; the core socket must no longer own it.
    socket.move_fd();
    Ok(())
}

/// Construct the PHP object over an already allocated core socket, which it owns.
pub(super) fn make_object(socket: NewSocket) -> Result<Owned, Bailout> {
    // SAFETY: the registered class's data layout is zero-initializable.
    let object = unsafe { SocketObject::try_create(&SOCKET, SOCKET.entry()) }?;
    // SAFETY: the new object, whose one reference the value takes.
    let object = unsafe { Owned::object(object) };
    let sock = Sock::of(&object).unwrap_or_else(|| unreachable!("a socket object"));
    sock.adopt(socket);
    Ok(object)
}

/// The object over a new core socket, or None after a warning when it has no
/// descriptor.
fn checked_object(socket: NewSocket) -> Result<Option<Owned>, Bailout> {
    if socket.co().fd() < 0 {
        let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        crate::net::warn_sys("new Socket() failed", error);
        if swoole_rs_zend::bailout::pending() {
            return Err(Bailout);
        }
        socket.free()?;
        return Ok(None);
    }
    make_object(socket).map(Some)
}

/// A new socket object of a native Swoole socket kind.
pub(crate) fn create_kind(kind: i32) -> Result<Option<Owned>, Bailout> {
    checked_object(native::make(-1, 0, kind, 0, false)?)
}

/// A new socket object that owns `fd`, of a native Swoole socket kind.
/// # Safety
/// The caller gives up `fd`.
pub(crate) unsafe fn from_fd(fd: i32, kind: i32) -> Result<Option<Owned>, Bailout> {
    checked_object(native::make(fd, 0, kind, 0, false)?)
}

/// An object over a duplicate of `fd`, which stays borrowed.
pub(super) fn duplicate(fd: i32, kind: i32) -> Result<Option<Owned>, Bailout> {
    native::check_reactor()?;
    // SAFETY: dup borrows the source descriptor and creates an independent owned descriptor.
    let duplicate = unsafe { libc::dup(fd) };
    if duplicate < 0 {
        let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        crate::net::warn_sys(&format!("dup({fd}) failed"), error);
        return if swoole_rs_zend::bailout::pending() {
            Err(Bailout)
        } else {
            Ok(None)
        };
    }
    // SAFETY: the new duplicate, which nothing else owns.
    unsafe { from_fd(duplicate, kind) }
}

/// Import retains the stream reference and its original descriptor mode after object creation.
/// # Safety
/// `fd` is the stream's descriptor, which the object takes over.
pub(crate) unsafe fn import_stream(
    fd: i32,
    kind: i32,
    nonblock: i32,
    stream: &Value,
) -> Result<Option<Owned>, Bailout> {
    // SAFETY: per the caller.
    let Some(object) = (unsafe { from_fd(fd, kind) })? else {
        return Ok(None);
    };
    let sock = Sock::of(&object).unwrap_or_else(|| unreachable!("a socket object"));
    sock.hold_stream(stream.copy());
    // Preserve the original one-bit nonblock assignment, rather than normalize other bits.
    if let Some(net) = sock.co().and_then(Co::net) {
        net.edit(|record| record.output.nonblock = u8::from(nonblock & 1 != 0));
    }
    Ok(Some(object))
}

/// The PHP constructor's complete core allocation/error path.
#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream narrows each zend_long to the C int constructor on Linux/macOS"
)]
pub(super) fn construct_core(
    domain: i64,
    kind: i64,
    protocol: i64,
) -> Result<Result<NewSocket, i32>, Bailout> {
    native::check_reactor()?;
    let socket = native::make(-1, domain as i32, kind as i32, protocol as i32, true)?;
    // Copy the descriptor before destruction can change errno.
    if socket.co().fd() >= 0 {
        return Ok(Ok(socket));
    }
    let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    socket.free()?;
    Ok(Err(error))
}
