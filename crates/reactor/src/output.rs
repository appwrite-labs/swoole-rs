//! `network::Socket`'s queued output: `handle_send` sends the front data chunk and
//! classifies the result; `handle_sendfile` corks, then classifies and completes
//! the front file request around native code's sendfile call (where the optional
//! halfclose repair sits); `sendfile_async` queues a file. Native code owns the
//! buffer, its chunks and the requests, and performs each effect through
//! [`OutputHooks`].
use std::io::Error;
use std::os::raw::{c_int, c_void};

/// `swReturnCode`: `SW_WAIT`, `SW_CLOSE`, `SW_ERROR`; and `SW_OK`/`SW_ERR`.
const WAIT: c_int = 2;
const CLOSE: c_int = 3;
const ERROR: c_int = 4;
const OK: c_int = 0;
const ERR: c_int = -1;

/// What queued output needs from its native `Socket`.
#[repr(C)]
pub struct OutputHooks {
    pub socket: *mut c_void,
    /// `Socket::kernel_nobufs`, which `catch_error` sets on ENOBUFS.
    pub kernel_nobufs: *mut bool,
    /// `Socket::send(data, length, 0)`.
    pub send:
        unsafe extern "C" fn(socket: *mut c_void, data: *const c_void, length: usize) -> isize,
    pub cork: unsafe extern "C" fn(socket: *mut c_void) -> bool,
    pub uncork: unsafe extern "C" fn(socket: *mut c_void),
    /// Records the send time when the socket has a send timer.
    pub sent: unsafe extern "C" fn(socket: *mut c_void),
}

/// What native code applies after `handle_send`/`handle_sendfile`: pop the front
/// chunk, store `offset` into it, set `close_wait`/`send_wait`, and warn (1 the data
/// send failed, 2 the sendfile of `requested` bytes failed). `code` is returned.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputResult {
    pub code: c_int,
    pub pop: bool,
    pub set_offset: bool,
    pub offset: u32,
    pub close_wait: bool,
    pub send_wait: bool,
    pub warn: c_int,
    pub requested: usize,
}

fn classify(hooks: &OutputHooks) -> c_int {
    let error = Error::last_os_error().raw_os_error().unwrap_or(0);
    // SAFETY: the caller's kernel_nobufs flag.
    unsafe { crate::swr_v1_socket_catch_error(error, hooks.kernel_nobufs) }
}

/// `Socket::handle_send`: send what is left of the front data chunk, `length`
/// bytes at `data` of which `offset` are already sent.
///
/// # Safety
/// `hooks` is valid for this call and `data` denotes `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_handle_send(
    hooks: *const OutputHooks,
    data: *const u8,
    length: u32,
    offset: u32,
) -> OutputResult {
    // SAFETY: the caller lends its hooks for this call.
    let hooks = unsafe { &*hooks };
    let mut result = OutputResult::default();
    let rest = length.wrapping_sub(offset);
    if rest == 0 {
        result.pop = true;
        return result;
    }
    // SAFETY: the unsent tail of the caller's chunk.
    let sent = unsafe {
        (hooks.send)(
            hooks.socket,
            data.wrapping_add(offset as usize).cast(),
            rest as usize,
        )
    };
    if sent < 0 {
        match classify(hooks) {
            ERROR => result.warn = 1,
            CLOSE => (result.close_wait, result.code) = (true, ERR),
            WAIT => (result.send_wait, result.code) = (true, ERR),
            _ => {}
        }
        return result;
    }
    let sent = sent.cast_unsigned();
    if sent == rest as usize {
        result.pop = true;
        return result;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream adds ssize_t to uint32_t offset"
    )]
    let advanced = offset.wrapping_add(sent as u32);
    (result.set_offset, result.offset) = (true, advanced);
    if sent < rest as usize {
        (result.send_wait, result.code) = (true, ERR);
    }
    result
}

/// `Socket::handle_sendfile` before its send: cork once, when more than `cork_min`
/// bytes of `[begin, end)` remain. `*corked` is the request's 0 unset / 1 / -1.
///
/// # Safety
/// `hooks` is valid for this call and `corked` is the request's writable field.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_sendfile_cork(
    hooks: *const OutputHooks,
    corked: *mut i8,
    begin: libc::off_t,
    end: libc::off_t,
    cork_min: libc::off_t,
) {
    // SAFETY: the caller lends its hooks and request field for this call.
    let hooks = unsafe { &*hooks };
    // SAFETY: the caller lends the stable cork slot; copy before the native effect.
    if unsafe { *corked } == 0 {
        // SAFETY: the caller's socket.
        let next = if end - begin > cork_min && unsafe { (hooks.cork)(hooks.socket) } {
            1
        } else {
            -1
        };
        // SAFETY: the effect returned and the caller still lends this slot exclusively.
        unsafe { *corked = next };
    }
}

/// `Socket::handle_sendfile` after native code sent `requested` bytes with result
/// `sent`, leaving the request at `[begin, end)`: classify a failure, record the
/// send time, and uncork and pop once the request is done.
///
/// # Safety
/// `hooks` is valid for this call and `corked` is the request's writable field.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_sendfile_sent(
    hooks: *const OutputHooks,
    sent: isize,
    requested: usize,
    corked: *mut i8,
    begin: libc::off_t,
    end: libc::off_t,
) -> OutputResult {
    // SAFETY: the caller lends its hooks and request field for this call.
    let hooks = unsafe { &*hooks };
    let result = OutputResult::default();
    if sent <= 0 {
        match classify(hooks) {
            ERROR => {
                return OutputResult {
                    warn: 2,
                    requested,
                    pop: true,
                    ..result
                };
            }
            CLOSE => {
                return OutputResult {
                    code: ERR,
                    close_wait: true,
                    ..result
                };
            }
            WAIT => {
                return OutputResult {
                    code: ERR,
                    send_wait: true,
                    ..result
                };
            }
            _ => {}
        }
    } else {
        // SAFETY: the caller's socket.
        unsafe { (hooks.sent)(hooks.socket) };
    }
    if begin != end {
        return result;
    }
    // SAFETY: the caller lends a live cork slot; no borrow crosses uncork.
    if unsafe { *corked } == 1 {
        // SAFETY: the caller's socket.
        unsafe { (hooks.uncork)(hooks.socket) };
        // SAFETY: uncork returned; the caller still lends the stable slot.
        unsafe { *corked = 0 };
    }
    OutputResult {
        pop: true,
        ..result
    }
}

/// `Socket::sendfile_async`: `prepare` makes the request and checks its range
/// (false fails the call, discarding it); then `buffer` makes the output buffer
/// if there is none and `enqueue` hands the request to it.
///
/// # Safety
/// Each callback is valid for `socket`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_sendfile_async(
    socket: *mut c_void,
    prepare: unsafe extern "C" fn(socket: *mut c_void) -> bool,
    buffer: unsafe extern "C" fn(socket: *mut c_void),
    enqueue: unsafe extern "C" fn(socket: *mut c_void),
) -> c_int {
    // SAFETY: the caller's request construction and check.
    if !unsafe { prepare(socket) } {
        return ERR;
    }
    // SAFETY: the caller's buffer and queue effects, in upstream's order.
    unsafe { buffer(socket) };
    // SAFETY: as above.
    unsafe { enqueue(socket) };
    OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        sends: Vec<usize>,
        supplied: Vec<u8>,
        each: isize,
        errno: c_int,
        corks: u32,
        uncorks: u32,
        sent: u32,
        cork_ok: bool,
    }

    thread_local! {
        static FAKE: RefCell<Fake> = RefCell::new(Fake::default());
    }

    fn set_errno(value: c_int) {
        #[cfg(target_os = "linux")]
        // SAFETY: this thread's errno slot.
        unsafe {
            *libc::__errno_location() = value;
        }
        #[cfg(not(target_os = "linux"))]
        // SAFETY: this thread's errno slot.
        unsafe {
            *libc::__error() = value;
        }
    }

    unsafe extern "C" fn send(_: *mut c_void, data: *const c_void, length: usize) -> isize {
        FAKE.with(|fake| {
            let mut fake = fake.borrow_mut();
            fake.sends.push(length);
            fake.supplied =
                unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) }.to_vec();
            set_errno(fake.errno);
            if fake.each < 0 {
                -1
            } else {
                fake.each.min(length.cast_signed())
            }
        })
    }
    unsafe extern "C" fn cork(_: *mut c_void) -> bool {
        FAKE.with(|fake| {
            fake.borrow_mut().corks += 1;
            fake.borrow().cork_ok
        })
    }
    unsafe extern "C" fn uncork(_: *mut c_void) {
        FAKE.with(|fake| fake.borrow_mut().uncorks += 1);
    }
    unsafe extern "C" fn sent(_: *mut c_void) {
        FAKE.with(|fake| fake.borrow_mut().sent += 1);
    }

    fn hooks(socket: *mut c_void, nobufs: &mut bool) -> OutputHooks {
        OutputHooks {
            socket,
            kernel_nobufs: nobufs,
            send,
            cork,
            uncork,
            sent,
        }
    }

    fn reset(each: isize, errno: c_int, cork_ok: bool) {
        FAKE.with(|fake| {
            *fake.borrow_mut() = Fake {
                each,
                errno,
                cork_ok,
                ..Fake::default()
            }
        });
    }

    #[test]
    fn handle_send_rules() {
        let mut nobufs = false;
        let hooks = hooks(std::ptr::null_mut(), &mut nobufs);
        let data = [0_u8; 10];
        let run = |each, errno, length, offset| {
            reset(each, errno, false);
            // SAFETY: local hooks and data.
            unsafe { swr_v1_socket_handle_send(&raw const hooks, data.as_ptr(), length, offset) }
        };
        assert_eq!(
            run(5, 0, 10, 10),
            OutputResult {
                pop: true,
                ..Default::default()
            },
            "nothing left"
        );
        assert_eq!(
            run(5, 0, 10, 5),
            OutputResult {
                pop: true,
                ..Default::default()
            },
            "the rest went"
        );
        assert_eq!(
            run(2, 0, 10, 5),
            OutputResult {
                code: ERR,
                set_offset: true,
                offset: 7,
                send_wait: true,
                ..Default::default()
            },
            "short write"
        );
        assert_eq!(
            run(-1, libc::EAGAIN, 10, 0),
            OutputResult {
                code: ERR,
                send_wait: true,
                ..Default::default()
            }
        );
        assert_eq!(
            run(-1, libc::EPIPE, 10, 0),
            OutputResult {
                code: ERR,
                close_wait: true,
                ..Default::default()
            }
        );
        assert_eq!(
            run(-1, libc::EINVAL, 10, 0),
            OutputResult {
                warn: 1,
                ..Default::default()
            }
        );
    }

    #[test]
    fn handle_sendfile_rules() {
        let mut nobufs = false;
        let hooks = hooks(std::ptr::null_mut(), &mut nobufs);
        let mut corked: i8 = 0;
        reset(0, 0, true);
        // 100,000 bytes from 0 corks once; a partial send keeps the request.
        // SAFETY: local hooks and request field.
        unsafe {
            swr_v1_socket_sendfile_cork(&raw const hooks, &raw mut corked, 0, 100_000, 65_536);
        }
        // SAFETY: as above.
        unsafe {
            swr_v1_socket_sendfile_cork(&raw const hooks, &raw mut corked, 65_536, 100_000, 65_536);
        }
        // SAFETY: as above.
        let partial = unsafe {
            swr_v1_socket_sendfile_sent(
                &raw const hooks,
                65_536,
                65_536,
                &raw mut corked,
                65_536,
                100_000,
            )
        };
        assert_eq!((partial, corked), (OutputResult::default(), 1));
        // SAFETY: as above.
        let done = unsafe {
            swr_v1_socket_sendfile_sent(
                &raw const hooks,
                34_464,
                34_464,
                &raw mut corked,
                100_000,
                100_000,
            )
        };
        assert_eq!(
            (done, corked),
            (
                OutputResult {
                    pop: true,
                    ..Default::default()
                },
                0
            )
        );
        FAKE.with(|fake| {
            let fake = fake.borrow();
            assert_eq!((fake.corks, fake.uncorks, fake.sent), (1, 1, 2));
        });

        // A small file never corks; EAGAIN waits; an error warns and pops.
        let mut corked: i8 = 0;
        reset(0, 0, true);
        // SAFETY: as above.
        unsafe { swr_v1_socket_sendfile_cork(&raw const hooks, &raw mut corked, 0, 10, 65_536) };
        assert_eq!(corked, -1);
        set_errno(libc::EAGAIN);
        // SAFETY: as above.
        let waited = unsafe {
            swr_v1_socket_sendfile_sent(&raw const hooks, -1, 10, &raw mut corked, 0, 10)
        };
        assert_eq!(
            waited,
            OutputResult {
                code: ERR,
                send_wait: true,
                ..Default::default()
            }
        );
        set_errno(libc::EINVAL);
        // SAFETY: as above.
        let failed = unsafe {
            swr_v1_socket_sendfile_sent(&raw const hooks, -1, 10, &raw mut corked, 0, 10)
        };
        assert_eq!(
            failed,
            OutputResult {
                pop: true,
                warn: 2,
                requested: 10,
                ..Default::default()
            }
        );
        FAKE.with(|fake| assert_eq!(fake.borrow().corks, 0));
    }

    #[test]
    fn sendfile_async_order() {
        unsafe extern "C" fn accept(socket: *mut c_void) -> bool {
            // SAFETY: the test's log.
            unsafe { (*socket.cast::<Vec<u8>>()).push(b'p') };
            true
        }
        unsafe extern "C" fn refuse(socket: *mut c_void) -> bool {
            // SAFETY: the test's log.
            unsafe { (*socket.cast::<Vec<u8>>()).push(b'p') };
            false
        }
        unsafe extern "C" fn buffer(socket: *mut c_void) {
            // SAFETY: the test's log.
            unsafe { (*socket.cast::<Vec<u8>>()).push(b'b') };
        }
        unsafe extern "C" fn enqueue(socket: *mut c_void) {
            // SAFETY: the test's log.
            unsafe { (*socket.cast::<Vec<u8>>()).push(b'e') };
        }
        let mut log: Vec<u8> = Vec::new();
        let socket = (&raw mut log).cast();
        // SAFETY: the callbacks write to the local log.
        assert_eq!(
            unsafe { swr_v1_socket_sendfile_async(socket, accept, buffer, enqueue) },
            OK
        );
        // SAFETY: as above.
        assert_eq!(
            unsafe { swr_v1_socket_sendfile_async(socket, refuse, buffer, enqueue) },
            ERR
        );
        assert_eq!(log, b"pbep");
    }
    // The 53 ordinary compositions from the old output.cc source-body facade.
    // Call the shipped Rust policies directly; the native buffer/request effects
    // are scripted here, while production_supported checks the real C++ binding.
    #[test]
    fn ordinary_data_positive_matrix() {
        let data = b"abcdef\0ghijklmnop";
        let mut nobufs = false;
        let hooks = hooks(std::ptr::null_mut(), &mut nobufs);
        let mut cases = 0;
        for remaining in [1_u32, 7, 13] {
            for sent in 1..=remaining {
                reset(isize::try_from(sent).unwrap(), 0, false);
                let result = unsafe {
                    swr_v1_socket_handle_send(&raw const hooks, data.as_ptr(), remaining + 3, 3)
                };
                assert_eq!(result.code, if sent == remaining { OK } else { ERR });
                FAKE.with_borrow(|fake| {
                    assert_eq!(fake.supplied, data[3..3 + remaining as usize]);
                    assert_eq!(fake.sends.len(), 1);
                });
                assert_eq!(result.pop, sent == remaining);
                assert_eq!(result.send_wait, sent < remaining);
                if sent < remaining {
                    assert_eq!(result.offset, 3 + sent);
                }
                assert!(!result.close_wait);
                cases += 1;
            }
        }

        assert_eq!(cases, 21);
    }

    struct Request {
        fd: c_int,
        length: usize,
        begin: libc::off_t,
        end: libc::off_t,
        buffer: bool,
        occupied: bool,
        timer: bool,
        last_sent_time: u64,
        sends: u32,
        destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    }
    unsafe extern "C" fn timed_sent(context: *mut c_void) {
        unsafe { sent(context) };
        let request = unsafe { &mut *context.cast::<Request>() };
        if request.timer {
            request.last_sent_time = 123;
        }
    }
    unsafe extern "C" fn prepare(context: *mut c_void) -> bool {
        let request = unsafe { &mut *context.cast::<Request>() };
        unsafe {
            crate::swr_v1_socket_sendfile_range(
                request.fd,
                request.begin,
                request.length,
                &raw mut request.end,
            ) == 0
        }
    }
    unsafe extern "C" fn buffer(context: *mut c_void) {
        unsafe { (*context.cast::<Request>()).buffer = true };
    }
    unsafe extern "C" fn dispose(context: *mut c_void) {
        unsafe { (*context.cast::<Request>()).occupied = false };
    }
    unsafe extern "C" fn enqueue(context: *mut c_void) {
        let request = unsafe { &mut *context.cast::<Request>() };
        assert!(request.buffer && !request.occupied);
        request.occupied = true;
        request.destroy = Some(dispose);
    }

    #[test]
    fn ordinary_file_positive_matrix() {
        use std::io::Write;
        use std::os::fd::AsRawFd;

        let path = std::env::temp_dir().join(format!("swr-ordinary-output-{}", std::process::id()));
        let mut file = std::fs::File::create(&path).unwrap();
        assert_eq!(file.write(b"0123456789012345678901234567890").unwrap(), 31);
        let mut nobufs = false;
        let hooks = hooks(std::ptr::null_mut(), &mut nobufs);
        let mut cases = 0;
        for existing in [false, true] {
            for length in [0_usize, 1, 7, 19] {
                for timer in [false, true] {
                    for cork_ok in [false, true] {
                        let file = std::fs::File::open(&path).unwrap();
                        let mut request = Request {
                            fd: file.as_raw_fd(),
                            length,
                            begin: 2,
                            end: 0,
                            buffer: existing,
                            occupied: false,
                            timer,
                            last_sent_time: 0,
                            sends: 0,
                            destroy: None,
                        };
                        let context = (&raw mut request).cast();
                        let file_hooks = OutputHooks {
                            socket: context,
                            sent: timed_sent,
                            ..hooks
                        };
                        assert_eq!(
                            unsafe {
                                swr_v1_socket_sendfile_async(context, prepare, buffer, enqueue)
                            },
                            OK
                        );
                        assert!(request.buffer);
                        let actual_length = if length == 0 { 29 } else { length };
                        assert_eq!(
                            (request.begin, request.end),
                            (2, libc::off_t::try_from(actual_length + 2).unwrap())
                        );
                        assert!(std::ptr::fn_addr_eq(
                            request.destroy.unwrap(),
                            dispose as unsafe extern "C" fn(*mut c_void)
                        ));
                        let mut remaining = actual_length;
                        let (mut iterations, mut popped, mut corked) = (0, 0, 0);
                        reset(0, 0, cork_ok);
                        while remaining != 0 {
                            unsafe {
                                swr_v1_socket_sendfile_cork(
                                    &raw const file_hooks,
                                    &raw mut corked,
                                    request.begin,
                                    request.end,
                                    8,
                                );
                            };
                            let sent = 3.min(remaining.min(7));
                            let requested =
                                crate::swr_v1_socket_sendfile_chunk(request.begin, request.end);
                            assert!(sent <= requested);
                            request.sends += 1;
                            request.begin += libc::off_t::try_from(sent).unwrap();
                            remaining -= sent;
                            let result = unsafe {
                                swr_v1_socket_sendfile_sent(
                                    &raw const file_hooks,
                                    isize::try_from(sent).unwrap(),
                                    requested,
                                    &raw mut corked,
                                    request.begin,
                                    request.end,
                                )
                            };
                            assert_eq!(result.code, OK);
                            if result.pop {
                                unsafe { request.destroy.unwrap()(context) };
                                popped += 1;
                            }
                            assert!(!result.send_wait && !result.close_wait);
                            iterations += 1;
                            if remaining != 0 {
                                assert_eq!(
                                    request.begin,
                                    libc::off_t::try_from(actual_length + 2 - remaining).unwrap()
                                );
                                assert_eq!(
                                    request.end,
                                    libc::off_t::try_from(actual_length + 2).unwrap()
                                );
                            }
                        }
                        FAKE.with_borrow(|fake| {
                            assert_eq!(fake.sent, iterations);
                            assert_eq!(fake.corks, u32::from(actual_length > 8));
                            assert_eq!(fake.uncorks, u32::from(actual_length > 8 && cork_ok));
                        });
                        assert_eq!(request.sends, iterations);
                        assert_eq!(popped, 1);
                        assert!(!request.occupied);
                        assert_eq!(request.last_sent_time, if timer { 123 } else { 0 });
                        cases += 1;
                    }
                }
            }
        }
        std::fs::remove_file(path).unwrap();
        assert_eq!(cases, 32);
    }
}
