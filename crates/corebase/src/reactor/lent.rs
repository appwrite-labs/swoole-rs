//! The buffer of one I/O call as a typed handle: [`Fill`] and [`Drain`], which
//! corebase defines with the calls over them.

/// The typed calls in the shape the tests' raw buffers and C-style TLS stubs have.
#[cfg(test)]
pub(crate) mod raw {
    use crate::lent::{Count, Out, Spare, Wrote};
    use crate::reactor::{Drain, Fill};
    use crate::reactor::{Socket, SteadyMs, record_peek, record_recv, record_send};
    use std::ffi::{c_int, c_void};

    fn count(n: isize) -> Option<usize> {
        usize::try_from(n).ok()
    }

    /// A TLS call on `socket` in C's shape: what `ssl_send` or `ssl_recv` returns.
    pub(crate) type TlsIo =
        unsafe extern "C" fn(socket: *mut c_void, buffer: *mut c_void, length: usize) -> isize;

    pub(crate) unsafe fn socket_record_send(
        socket: Socket<'_>,
        buffer: *const c_void,
        length: usize,
        flags: c_int,
        tls: TlsIo,
        now: SteadyMs,
    ) -> isize {
        let data = unsafe { Drain::from_raw(buffer, length) };
        let tls = |socket: Socket<'_>, data: Drain<'_>| {
            count(unsafe { tls(socket.as_ptr().cast(), data.as_ptr().cast_mut(), data.len()) })
        };
        record_send(socket, data, flags, tls, now).map_or(-1, usize::cast_signed)
    }

    /// `tls` over a typed buffer.
    pub(crate) fn fills(tls: TlsIo) -> impl Fn(Socket<'_>, Fill<'_>) -> Option<usize> {
        move |socket, mut fill| {
            let slots = fill.slots();
            count(unsafe {
                tls(
                    socket.as_ptr().cast(),
                    slots.as_mut_ptr().cast(),
                    slots.len(),
                )
            })
        }
    }

    pub(crate) unsafe fn socket_record_recv(
        socket: Socket<'_>,
        buffer: *mut c_void,
        length: usize,
        flags: c_int,
        tls: TlsIo,
        now: SteadyMs,
    ) -> isize {
        // The stubs write what they return, as the C TLS reads do: the same call,
        // with the tokens' layout and its result as what it wrote.
        let tls: for<'id> unsafe extern "C" fn(*mut c_void, Out<'id>, Count<'id>) -> Wrote<'id> =
            unsafe { std::mem::transmute(tls) };
        let tls = |socket: Socket<'_>, spare: &mut Spare<'_>| {
            count(spare.lend(|at, n| unsafe { tls(socket.as_ptr().cast(), at, n) }))
        };
        let mut buffer = unsafe { Fill::from_raw(buffer, length) };
        let received = buffer.spare(|spare| record_recv(socket, spare, flags, tls, now));
        received.map_or(-1, usize::cast_signed)
    }

    pub(crate) unsafe fn socket_record_peek(
        socket: Socket<'_>,
        buffer: *mut c_void,
        length: usize,
        flags: c_int,
        tls: TlsIo,
    ) -> isize {
        let buffer = unsafe { Fill::from_raw(buffer, length) };
        record_peek(socket, buffer, flags, fills(tls)).map_or(-1, usize::cast_signed)
    }
}
