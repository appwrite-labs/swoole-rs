//! The backends' kernel registration (upstream `ReactorEpoll`/`ReactorKqueue`
//! `add`, `set` and `del`): the event-mask translation and the `epoll_ctl` or
//! `kevent` calls in upstream's order. Each returns which call failed, with `errno`
//! as the kernel left it, for `swr_v1_reactor_change`'s warnings.
use std::ffi::{c_int, c_void};

const SW_EVENT_DEAULT: c_int = 1 << 8;
const SW_EVENT_READ: c_int = 1 << 9;
const SW_EVENT_WRITE: c_int = 1 << 10;

/// `Reactor::isset_read_event`.
fn reads(events: c_int) -> bool {
    events < SW_EVENT_DEAULT || events & SW_EVENT_READ != 0
}

fn writes(events: c_int) -> bool {
    events & SW_EVENT_WRITE != 0
}

#[cfg(target_os = "linux")]
mod epoll {
    use super::{c_int, c_void, reads, writes};

    const SW_EVENT_ERROR: c_int = 1 << 11;
    const SW_EVENT_ONCE: c_int = 1 << 12;

    /// `ReactorEpoll::get_events`.
    #[expect(
        clippy::cast_sign_loss,
        reason = "the EPOLL* flags are non-negative bit masks"
    )]
    fn interest(events: c_int) -> u32 {
        let mut interest = 0;
        if reads(events) {
            interest |= libc::EPOLLIN;
        }
        if writes(events) {
            interest |= libc::EPOLLOUT;
        }
        if events & SW_EVENT_ONCE != 0 {
            interest |= libc::EPOLLONESHOT;
        }
        if events & SW_EVENT_ERROR != 0 {
            interest |= libc::EPOLLRDHUP | libc::EPOLLHUP | libc::EPOLLERR;
        }
        interest as u32
    }

    /// `epoll_ctl` for add (0), set (1) or del (2) of `fd` with `socket` as its token;
    /// the kernel's result.
    ///
    /// # Safety
    /// `socket` stays valid while registered; the wait loop hands it back to native.
    pub(crate) unsafe fn epoll_ctl(
        epfd: c_int,
        op: u32,
        fd: c_int,
        socket: *mut c_void,
        events: c_int,
    ) -> c_int {
        let op = match op {
            0 => libc::EPOLL_CTL_ADD,
            1 => libc::EPOLL_CTL_MOD,
            _ => libc::EPOLL_CTL_DEL,
        };
        let mut event = libc::epoll_event {
            events: interest(events),
            u64: socket as usize as u64,
        };
        let event = if op == libc::EPOLL_CTL_DEL {
            std::ptr::null_mut()
        } else {
            &raw mut event
        };
        // SAFETY: `event` is null (DEL) or a valid record for the call.
        unsafe { libc::epoll_ctl(epfd, op, fd, event) }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{SW_EVENT_READ, SW_EVENT_WRITE};
        use super::*;

        #[test]
        fn translates_and_registers() {
            assert_eq!(interest(0), libc::EPOLLIN as u32);
            assert_eq!(
                interest(SW_EVENT_WRITE | SW_EVENT_ONCE),
                (libc::EPOLLOUT | libc::EPOLLONESHOT) as u32
            );
            let mut fds = [0; 2];
            unsafe {
                assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
                let epfd = libc::epoll_create1(0);
                let token = 7 as *mut c_void;
                assert_eq!(epoll_ctl(epfd, 0, fds[0], token, SW_EVENT_READ), 0);
                assert_eq!(epoll_ctl(epfd, 1, fds[0], token, SW_EVENT_WRITE), 0);
                assert_eq!(epoll_ctl(epfd, 2, fds[0], token, 0), 0);
                assert_eq!(epoll_ctl(epfd, 2, fds[0], token, 0), -1);
                libc::close(epfd);
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use epoll::*;

#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
pub(crate) use kqueue::*;

#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
mod kqueue {
    use super::{SW_EVENT_READ, SW_EVENT_WRITE, c_int, c_void, reads, writes};

    /// `NOTE_EOF` where the platform defines it (upstream's `#ifdef NOTE_EOF`): only
    /// OpenBSD's headers do; FreeBSD's 0x0002 is `NOTE_FILE_POLL`.
    #[cfg(target_os = "openbsd")]
    const NOTE_EOF: u32 = libc::NOTE_EOF;
    #[cfg(not(target_os = "openbsd"))]
    const NOTE_EOF: u32 = 0;

    /// One change for `fd`: `EV_ADD` or `EV_DELETE` on `filter`; whether it took.
    fn change(
        kq: c_int,
        fd: c_int,
        filter: i16,
        flags: u16,
        fflags: u32,
        socket: *mut c_void,
    ) -> bool {
        let mut event: libc::kevent = swoole_rs_corebase::sys::zeroed();
        // Upstream's `EV_SET` converts the int fd to `uintptr_t` the same way.
        #[expect(clippy::cast_sign_loss, reason = "as upstream's EV_SET")]
        let ident = fd as isize as usize;
        event.ident = ident;
        event.filter = filter;
        event.flags = flags;
        event.fflags = fflags;
        event.udata = socket.cast();
        // SAFETY: one valid change record, no output records.
        let result = unsafe {
            libc::kevent(
                kq,
                &raw const event,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        result >= 0
    }

    type Filter = fn(c_int, c_int, u16, *mut c_void) -> bool;

    fn read(kq: c_int, fd: c_int, flags: u16, socket: *mut c_void) -> bool {
        let fflags = if flags == libc::EV_ADD { NOTE_EOF } else { 0 };
        change(kq, fd, libc::EVFILT_READ, flags, fflags, socket)
    }

    fn write(kq: c_int, fd: c_int, flags: u16, socket: *mut c_void) -> bool {
        change(kq, fd, libc::EVFILT_WRITE, flags, 0, socket)
    }

    /// `ReactorKqueue::add`'s kernel calls: 0, or 1 (read) or 2 (write) for the failed one.
    ///
    /// # Safety
    /// `socket` stays valid while registered; the wait loop hands it back to native.
    pub(crate) unsafe fn kqueue_add(
        kq: c_int,
        fd: c_int,
        socket: *mut c_void,
        events: c_int,
    ) -> c_int {
        if reads(events) && !read(kq, fd, libc::EV_ADD, socket) {
            return 1;
        }
        if writes(events) && !write(kq, fd, libc::EV_ADD, socket) {
            return 2;
        }
        0
    }

    /// `ReactorKqueue::set`'s kernel calls: 0, or the failed one: 1 read add, 2 read
    /// delete, 3 write add, 4 write delete.
    ///
    /// # Safety
    /// As for [`kqueue_add`].
    pub(crate) unsafe fn kqueue_set(
        kq: c_int,
        fd: c_int,
        socket: *mut c_void,
        events: c_int,
    ) -> c_int {
        let read_flags = if reads(events) {
            libc::EV_ADD
        } else {
            libc::EV_DELETE
        };
        if !read(kq, fd, read_flags, socket) {
            return if reads(events) { 1 } else { 2 };
        }
        let write_flags = if writes(events) {
            libc::EV_ADD
        } else {
            libc::EV_DELETE
        };
        if !write(kq, fd, write_flags, socket) {
            return if writes(events) { 3 } else { 4 };
        }
        0
    }

    /// `ReactorKqueue::del`'s kernel calls for the registered `events`. Each failed
    /// filter's errno goes in `errors` (read, write; 0 when it took); a failure other
    /// than EBADF or ENOENT stops there and returns -1, else 0.
    pub(crate) fn kqueue_del(
        kq: c_int,
        fd: c_int,
        socket: *mut c_void,
        events: c_int,
        errors: &mut [c_int; 2],
    ) -> c_int {
        *errors = [0; 2];
        let filters: [(bool, Filter); 2] = [
            (events & SW_EVENT_READ != 0, read),
            (events & SW_EVENT_WRITE != 0, write),
        ];
        for (slot, (registered, filter)) in errors.iter_mut().zip(filters) {
            if registered && !filter(kq, fd, libc::EV_DELETE, socket) {
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                *slot = errno;
                if errno != libc::EBADF && errno != libc::ENOENT {
                    return -1;
                }
            }
        }
        0
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn registers_in_upstream_order() {
            let mut fds = [0; 2];
            let mut errors = [0; 2];
            unsafe {
                assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
                let kq = libc::kqueue();
                let token = 7 as *mut c_void;
                assert_eq!(kqueue_add(kq, fds[0], token, SW_EVENT_READ), 0);
                assert_eq!(
                    kqueue_set(kq, fds[0], token, SW_EVENT_READ | SW_EVENT_WRITE),
                    0
                );
                assert_eq!(kqueue_set(kq, fds[0], token, SW_EVENT_READ), 0);
                // The write filter is gone, so deleting it again reports ENOENT and continues.
                let both = SW_EVENT_READ | SW_EVENT_WRITE;
                assert_eq!(kqueue_del(kq, fds[0], token, both, &mut errors), 0);
                assert_eq!(errors, [0, libc::ENOENT]);
                // A bad descriptor fails the first call.
                assert_eq!(kqueue_add(kq, -1, token, SW_EVENT_READ), 1);
                assert_eq!(kqueue_set(kq, -1, token, SW_EVENT_WRITE), 2);
                libc::close(kq);
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
        }
    }
}
