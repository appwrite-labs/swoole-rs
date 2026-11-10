//! Queued output over Rust socket and buffer records: the ordinary send commits
//! the front chunk's cursor, the socket's wait flags and the queue's detachment;
//! sendfile requests own their descriptor, path and range in `file_output`. The
//! socket's I/O and cork effects are the caller's [`OutputHooks`].
use std::io::Error;
use std::os::raw::{c_int, c_void};

/// `swReturnCode`: `SW_WAIT`, `SW_CLOSE`, `SW_ERROR`; and `SW_ERR`.
const WAIT: c_int = 2;
const CLOSE: c_int = 3;
const ERROR: c_int = 4;
const ERR: c_int = -1;

/// What queued output needs from its `Socket`.
#[derive(Clone, Copy)]
pub struct OutputHooks {
    socket: *mut c_void,
    /// `Socket::kernel_nobufs`, which `catch_error` sets on ENOBUFS.
    kernel_nobufs: *mut bool,
    /// `Socket::send(data, length, 0)`.
    send: unsafe extern "C" fn(socket: *mut c_void, data: *const c_void, length: usize) -> isize,
    cork: unsafe extern "C" fn(socket: *mut c_void) -> bool,
    uncork: unsafe extern "C" fn(socket: *mut c_void),
    /// Records the send time when the socket has a send timer.
    sent: unsafe extern "C" fn(socket: *mut c_void),
}

impl OutputHooks {
    /// # Safety
    /// While the hooks live, `kernel_nobufs` is writable and each effect may be
    /// called with `socket`, `send` with any readable bytes.
    pub const unsafe fn new(
        socket: *mut c_void,
        kernel_nobufs: *mut bool,
        send: unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> isize,
        cork: unsafe extern "C" fn(*mut c_void) -> bool,
        uncork: unsafe extern "C" fn(*mut c_void),
        sent: unsafe extern "C" fn(*mut c_void),
    ) -> Self {
        Self {
            socket,
            kernel_nobufs,
            send,
            cork,
            uncork,
            sent,
        }
    }

    fn send(&self, data: &[u8]) -> isize {
        // SAFETY: `new`'s contract, over the caller's readable bytes.
        unsafe { (self.send)(self.socket, data.as_ptr().cast(), data.len()) }
    }

    fn cork(&self) -> bool {
        // SAFETY: `new`'s contract.
        unsafe { (self.cork)(self.socket) }
    }

    fn uncork(&self) {
        // SAFETY: `new`'s contract.
        unsafe { (self.uncork)(self.socket) }
    }

    fn sent(&self) {
        // SAFETY: `new`'s contract.
        unsafe { (self.sent)(self.socket) }
    }

    /// `catch_error` over the errno the last effect left.
    fn classify(&self) -> c_int {
        let error = Error::last_os_error().raw_os_error().unwrap_or(0);
        // SAFETY: `new`'s writable flag, which no other borrow holds during this call.
        crate::catch_error(error, unsafe { &mut *self.kernel_nobufs })
    }
}

/// What the caller applies after `handle_send`/`handle_sendfile`: pop the front
/// chunk, store `offset` into it, set `close_wait`/`send_wait`, and warn (1 the data
/// send failed, 2 the sendfile of `requested` bytes failed). `code` is returned.
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

impl OutputResult {
    /// Sets the socket's `close_wait` and `send_wait` flags the result asks for. It
    /// borrows only the socket's output state, so it leaves pointers to the socket's
    /// other fields, such as the hooks' `kernel_nobufs`, usable.
    pub const fn publish(&self, output: &mut crate::SocketOutput) {
        if self.close_wait {
            output.close_wait = 1;
        }
        if self.send_wait {
            output.send_wait = 1;
        }
    }
}

/// `Socket::handle_send`: send what is left of the front data chunk `data`, of
/// which `offset` bytes are already sent.
#[must_use]
pub fn handle_send(hooks: &OutputHooks, data: &[u8], offset: u32) -> OutputResult {
    let mut result = OutputResult::default();
    let rest = data.get(offset as usize..).unwrap_or_default();
    if rest.is_empty() {
        result.pop = true;
        return result;
    }
    let sent = hooks.send(rest);
    if sent < 0 {
        match hooks.classify() {
            ERROR => result.warn = 1,
            CLOSE => (result.close_wait, result.code) = (true, ERR),
            WAIT => (result.send_wait, result.code) = (true, ERR),
            _ => {}
        }
        return result;
    }
    let sent = sent.cast_unsigned();
    if sent == rest.len() {
        result.pop = true;
        return result;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream adds ssize_t to uint32_t offset"
    )]
    let advanced = offset.wrapping_add(sent as u32);
    (result.set_offset, result.offset) = (true, advanced);
    if sent < rest.len() {
        (result.send_wait, result.code) = (true, ERR);
    }
    result
}

/// `Socket::handle_sendfile` before its send: cork once, when more than `cork_min`
/// bytes of `[begin, end)` remain. `corked` is the request's 0 unset / 1 / -1.
pub fn sendfile_cork(
    hooks: &OutputHooks,
    corked: &mut i8,
    begin: libc::off_t,
    end: libc::off_t,
    cork_min: libc::off_t,
) {
    if *corked == 0 {
        *corked = if end - begin > cork_min && hooks.cork() {
            1
        } else {
            -1
        };
    }
}

/// `Socket::handle_sendfile` after the caller sent `requested` bytes with result
/// `sent`, leaving the request at `[begin, end)`: classify a failure, record the
/// send time, and uncork and pop once the request is done.
#[must_use]
pub fn sendfile_sent(
    hooks: &OutputHooks,
    sent: isize,
    requested: usize,
    corked: &mut i8,
    begin: libc::off_t,
    end: libc::off_t,
) -> OutputResult {
    let result = OutputResult::default();
    if sent <= 0 {
        match hooks.classify() {
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
        hooks.sent();
    }
    if begin != end {
        return result;
    }
    if *corked == 1 {
        hooks.uncork();
        *corked = 0;
    }
    OutputResult {
        pop: true,
        ..result
    }
}

/// Completed queue mutation and an optional token for the caller to dispose.
pub struct OwnedOutputResult {
    pub result: OutputResult,
    pub status: i32,
    pub detached: swoole_rs_buffer::TypedDetached,
}

/// `Socket::handle_send` over the socket's typed queue: send its front, then
/// commit the cursor and the socket's wait flags, and detach a finished chunk.
///
/// # Safety
/// `socket` and `buffer` are live and exclusive apart from `hooks`' own use of the socket.
pub unsafe fn handle_send_owned(
    socket: *mut crate::SocketRecord,
    buffer: *mut swoole_rs_buffer::Buffer,
    hooks: &OutputHooks,
) -> OwnedOutputResult {
    let mut output = OwnedOutputResult {
        result: OutputResult::default(),
        status: 0,
        detached: swoole_rs_buffer::TypedDetached::default(),
    };
    let mut front = swoole_rs_buffer::TypedFront::default();
    // SAFETY: a live typed queue and disjoint local observation, with no retained borrow.
    output.status = unsafe { swoole_rs_buffer::swr_v1_buffer_typed_front(buffer, &raw mut front) };
    if output.status != 0 {
        return output;
    }
    let data = if front.data.is_null() {
        &[]
    } else {
        // SAFETY: the queue's front chunk, which it keeps until the pop below.
        unsafe { std::slice::from_raw_parts(front.data, front.length as usize) }
    };
    output.result = handle_send(hooks, data, front.offset);
    if output.result.set_offset {
        // SAFETY: the observed front identity is committed only after the effect returned.
        output.status = unsafe {
            swoole_rs_buffer::swr_v1_buffer_typed_consume(
                buffer,
                front.generation,
                front.offset,
                output.result.offset.wrapping_sub(front.offset),
            )
        };
        if output.status != 0 {
            return output;
        }
    }
    // SAFETY: the effects returned; the caller lends the socket exclusively.
    output.result.publish(unsafe { &mut (*socket).output });
    if output.result.pop {
        // SAFETY: detach transfers custody to the caller without invoking a disposer.
        output.status =
            unsafe { swoole_rs_buffer::swr_v1_buffer_typed_pop(buffer, &raw mut output.detached) };
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: c_int = 0;
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
    fn owned_queue_commits_offsets_wait_flags_and_detaches_after_effects() {
        unsafe {
            let socket = crate::SocketRecord::with_fd(-1, 6).into_raw();
            let mut queue = std::ptr::null_mut();
            assert_eq!(
                swoole_rs_buffer::swr_v1_buffer_typed_create(64, &raw mut queue),
                0
            );
            assert_eq!(
                swoole_rs_buffer::swr_v1_buffer_append(queue, b"hello".as_ptr(), 5),
                0
            );
            let hooks = hooks(socket.cast(), &mut (*socket).kernel_nobufs);
            reset(2, 0, false);
            let sent = handle_send_owned(socket, queue, &hooks);
            assert_eq!(
                (sent.status, sent.result.code, sent.result.offset),
                (0, -1, 2)
            );
            assert_eq!((*socket).output.send_wait, 1);
            let mut front = swoole_rs_buffer::TypedFront::default();
            assert_eq!(
                swoole_rs_buffer::swr_v1_buffer_typed_front(queue, &raw mut front),
                0
            );
            assert_eq!(front.offset, 2);
            reset(-1, libc::EPIPE, false);
            let closed = handle_send_owned(socket, queue, &hooks);
            assert_eq!(
                (
                    closed.status,
                    closed.result.code,
                    (*socket).output.close_wait
                ),
                (0, -1, 1)
            );
            assert_eq!(
                swoole_rs_buffer::swr_v1_buffer_typed_front(queue, &raw mut front),
                0
            );
            assert_eq!(front.offset, 2);
            reset(3, 0, false);
            let completed = handle_send_owned(socket, queue, &hooks);
            assert_eq!((completed.status, completed.result.pop), (0, true));
            assert!(completed.detached.token.is_null() && completed.detached.dispose.is_none());
            assert_eq!(
                swoole_rs_buffer::swr_v1_buffer_typed_front(queue, &raw mut front),
                1
            );
            assert_eq!(swoole_rs_buffer::swr_v1_buffer_release(queue), 0);
            crate::SocketRecord::free(socket);
        }
    }
    #[test]
    fn handle_send_rules() {
        let mut nobufs = false;
        let hooks = hooks(std::ptr::null_mut(), &mut nobufs);
        let data = [0_u8; 10];
        let run = |each, errno, length: u32, offset| {
            reset(each, errno, false);
            handle_send(&hooks, &data[..length as usize], offset)
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
        sendfile_cork(&hooks, &mut corked, 0, 100_000, 65_536);
        sendfile_cork(&hooks, &mut corked, 65_536, 100_000, 65_536);
        let partial = sendfile_sent(&hooks, 65_536, 65_536, &mut corked, 65_536, 100_000);
        assert_eq!((partial, corked), (OutputResult::default(), 1));
        let done = sendfile_sent(&hooks, 34_464, 34_464, &mut corked, 100_000, 100_000);
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
        sendfile_cork(&hooks, &mut corked, 0, 10, 65_536);
        assert_eq!(corked, -1);
        set_errno(libc::EAGAIN);
        let waited = sendfile_sent(&hooks, -1, 10, &mut corked, 0, 10);
        assert_eq!(
            waited,
            OutputResult {
                code: ERR,
                send_wait: true,
                ..Default::default()
            }
        );
        set_errno(libc::EINVAL);
        let failed = sendfile_sent(&hooks, -1, 10, &mut corked, 0, 10);
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

    // The 53 ordinary compositions from the old output.cc source-body facade.
    // Call the shipped Rust policies directly; the native buffer/request effects
    // are scripted here; the PHPT corpus and hooks-system-rust run the real binding.
    #[test]
    fn ordinary_data_positive_matrix() {
        let data = b"abcdef\0ghijklmnop";
        let mut nobufs = false;
        let hooks = hooks(std::ptr::null_mut(), &mut nobufs);
        let mut cases = 0;
        for remaining in [1_u32, 7, 13] {
            for sent in 1..=remaining {
                reset(isize::try_from(sent).unwrap(), 0, false);
                let result = handle_send(&hooks, &data[..(remaining + 3) as usize], 3);
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
        crate::sendfile_range(request.fd, request.begin, request.length, &mut request.end) == 0
    }
    unsafe extern "C" fn buffer(context: *mut c_void) {
        unsafe { (*context.cast::<Request>()).buffer = true };
    }
    unsafe extern "C" fn dispose(context: *mut c_void) {
        unsafe { (*context.cast::<Request>()).occupied = false };
    }
    /// One reified pointer: each cast of `dispose` may get its own address.
    static DISPOSE: unsafe extern "C" fn(*mut c_void) = dispose;
    unsafe extern "C" fn enqueue(context: *mut c_void) {
        let request = unsafe { &mut *context.cast::<Request>() };
        assert!(request.buffer && !request.occupied);
        request.occupied = true;
        request.destroy = Some(DISPOSE);
    }

    #[test]
    fn ordinary_file_positive_matrix() {
        use std::io::Write;
        use std::os::fd::AsRawFd;

        let path = std::env::temp_dir().join(format!("swr-ordinary-output-{}", std::process::id()));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"0123456789012345678901234567890").unwrap();
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
                        // `Socket::sendfile_async`'s order: check the range, make the
                        // buffer, queue the request.
                        assert!(unsafe { prepare(context) });
                        unsafe { buffer(context) };
                        unsafe { enqueue(context) };
                        assert!(request.buffer);
                        let actual_length = if length == 0 { 29 } else { length };
                        assert_eq!(
                            (request.begin, request.end),
                            (2, libc::off_t::try_from(actual_length + 2).unwrap())
                        );
                        assert!(std::ptr::fn_addr_eq(request.destroy.unwrap(), DISPOSE));
                        let mut remaining = actual_length;
                        let (mut iterations, mut popped, mut corked) = (0, 0, 0);
                        reset(0, 0, cork_ok);
                        while remaining != 0 {
                            sendfile_cork(&file_hooks, &mut corked, request.begin, request.end, 8);
                            let sent = 3.min(remaining.min(7));
                            let requested = crate::sendfile_chunk(request.begin, request.end);
                            assert!(sent <= requested);
                            // Through `context`: a write to `request` itself would end it.
                            let shared = context.cast::<Request>();
                            unsafe { (*shared).sends += 1 };
                            unsafe { (*shared).begin += libc::off_t::try_from(sent).unwrap() };
                            remaining -= sent;
                            let result = sendfile_sent(
                                &file_hooks,
                                isize::try_from(sent).unwrap(),
                                requested,
                                &mut corked,
                                request.begin,
                                request.end,
                            );
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
