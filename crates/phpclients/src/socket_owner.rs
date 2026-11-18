//! Object and imported-resource custody for Coroutine Socket.
use super::{Data, Sock};
use crate::native::{self, Co, NewSocket};
use swoole_rs_corebase::CCast;
use swoole_rs_zend::{Owned, Value, bailout::Bailout, new_object};

/// Free the length callback, then the owned core socket after releasing any
/// imported stream reference (whose destructor's bailout is deferred). Upstream's
/// free leaves the callback to the exit. Do not call close: shutdown can
/// otherwise resume other coroutines. The object's drop: nothing else holds it,
/// and no method remains suspended on it.
pub(super) fn release(data: &mut Data) -> Result<(), Bailout> {
    let Some(socket) = data.socket.take() else {
        return Ok(());
    };
    let co = socket.co();
    let freed = co.free_length_callable();
    if data.has_stream() {
        // The still-owned core socket relinquishes the fd before the stream closes it.
        co.move_fd();
        drop(data.stream.take());
    }
    freed.and(socket.free())
}

/// Free the length callback, then close an imported stream or the owned core socket.
pub(super) fn close(sock: Sock, socket: Co) -> Result<(), Bailout> {
    socket.free_length_callable()?;
    if !Owned::close_imported_stream(&sock.data().stream)? {
        return socket.close_trapped();
    }
    // The imported stream closed the fd; the core socket must no longer own it.
    socket.move_fd();
    Ok(())
}

/// Construct the PHP object over an already allocated core socket, which it
/// owns: when the allocation bails out, the socket goes with the data.
pub(super) fn make_object(socket: NewSocket) -> Result<Owned, Bailout> {
    let object = new_object(Data::owning(Some(socket)));
    Sock::of(&object).ok_or(Bailout)?.publish();
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

/// A new socket object of a native Swoole socket kind that owns `fd`, which the
/// caller gives up (as [`native::make`] takes any descriptor it is handed).
pub(crate) fn from_fd(fd: i32, kind: i32) -> Result<Option<Owned>, Bailout> {
    checked_object(native::make(fd, 0, kind, 0, false)?)
}

/// An object over a duplicate of `fd`, which stays borrowed.
pub(super) fn duplicate(fd: i32, kind: i32) -> Result<Option<Owned>, Bailout> {
    native::check_reactor()?;
    let duplicate = match swoole_rs_corebase::sys::dup(fd) {
        Ok(duplicate) => duplicate,
        Err(error) => {
            let error = error.raw_os_error().unwrap_or(0);
            crate::net::warn_sys(&format!("dup({fd}) failed"), error);
            return if swoole_rs_zend::bailout::pending() {
                Err(Bailout)
            } else {
                Ok(None)
            };
        }
    };
    // The new duplicate, which nothing else owns.
    from_fd(duplicate, kind)
}

/// Import retains the stream reference and its original descriptor mode after object creation.
/// `fd` is the stream's descriptor, which the object takes over.
pub(crate) fn import_stream(
    fd: i32,
    kind: i32,
    nonblock: i32,
    stream: &Value,
) -> Result<Option<Owned>, Bailout> {
    let Some(object) = from_fd(fd, kind)? else {
        return Ok(None);
    };
    let sock = Sock::of(&object).unwrap_or_else(|| unreachable!("a socket object"));
    sock.hold_stream(stream.copy());
    // Preserve the original one-bit nonblock assignment, rather than normalize other bits.
    if let Some(socket) = sock.co() {
        socket.with_net(|net| net.edit(|record| record.output.nonblock = nonblock & 1 != 0));
    }
    Ok(Some(object))
}

/// The PHP constructor's complete core allocation/error path.
pub(super) fn construct_core(
    domain: i64,
    kind: i64,
    protocol: i64,
) -> Result<Result<NewSocket, i32>, Bailout> {
    native::check_reactor()?;
    let socket = native::make(
        -1,
        domain.c_cast::<i32>(),
        kind.c_cast::<i32>(),
        protocol.c_cast::<i32>(),
        true,
    )?;
    // Copy the descriptor before destruction can change errno.
    if socket.co().fd() >= 0 {
        return Ok(Ok(socket));
    }
    let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    socket.free()?;
    Ok(Err(error))
}
