use super::*;
use std::{ffi::CString, net::TcpListener};
use swoole_rs_curl_sys::{CURLOPT_URL, curl_easy_cleanup, curl_easy_init};

thread_local! {
    /// What the fake host saw, and whether its event loop runs.
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static LOOP: Cell<bool> = const { Cell::new(true) };
}

fn log(entry: String) {
    LOG.with(|log| log.borrow_mut().push(entry));
}

fn take() -> Vec<String> {
    LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
}

/// The fake host the multi calls instead of `host.rs`'s.
pub(crate) mod host {
    use super::{LOOP, Multi, log};
    use std::ffi::{c_int, c_long, c_void};
    use std::{cell::Cell, ptr};

    /// A fake reactor socket: its events and whether it is out of the reactor.
    #[derive(Clone, Copy)]
    pub(crate) struct ReactorSocket(*mut [i32; 2]);

    impl ReactorSocket {
        pub(crate) fn new(fd: c_int) -> Self {
            log(format!("new {fd}"));
            Self(Box::into_raw(Box::new([0, 1])))
        }
        #[expect(
            clippy::unused_self,
            reason = "the fake reports events without an object"
        )]
        pub(crate) fn set_object(self, _object: *mut c_void) {}
        pub(crate) fn events(self) -> c_int {
            unsafe { (*self.0)[0] }
        }
        pub(crate) fn removed(self) -> bool {
            unsafe { (*self.0)[1] != 0 }
        }
        pub(crate) fn add(self, events: c_int) -> c_int {
            log(format!("add {events}"));
            unsafe { *self.0 = [events, 0] };
            0
        }
        pub(crate) fn set(self, events: c_int) -> c_int {
            log(format!("set {events}"));
            unsafe { (*self.0)[0] = events };
            0
        }
        pub(crate) fn del(self) -> c_int {
            log("del".into());
            unsafe { *self.0 = [0, 1] };
            0
        }
        pub(crate) unsafe fn free(self) {
            log("free".into());
            drop(unsafe { Box::from_raw(self.0) });
        }
    }

    pub(crate) fn event_available() -> bool {
        LOOP.with(Cell::get)
    }
    pub(crate) fn install_handlers() {}
    pub(crate) fn timer_available() -> bool {
        true
    }
    pub(crate) fn timer_add(ms: c_long, _multi: &Multi) -> *mut c_void {
        log(format!("timer {ms}"));
        ptr::dangling_mut::<u8>().cast()
    }
    pub(crate) unsafe fn timer_del(_timer: *mut c_void) {
        log("timer del".into());
    }
    pub(crate) fn current() -> *mut c_void {
        ptr::null_mut()
    }
    pub(crate) unsafe fn yield_(_co: *mut c_void, _timeout: f64) -> bool {
        false
    }
    pub(crate) unsafe fn resume(_co: *mut c_void) {}
    pub(crate) unsafe fn defer(_multi: *const Multi) {
        log("defer".into());
    }
}

/// An easy handle for `url`, with its Swoole handle.
fn easy(url: &str) -> *mut CURL {
    let cp = unsafe { curl_easy_init() };
    let url = CString::new(url).unwrap();
    unsafe { curl_easy_setopt(cp, CURLOPT_URL, url.as_ptr()) };
    unsafe { swr_v1_curl_create_handle(cp) };
    cp
}

fn close(cp: *mut CURL) {
    unsafe { swr_v1_curl_destroy_handle(cp) };
    assert!(unsafe { swr_v1_curl_get_handle(cp) }.is_null(), "destroyed");
    unsafe { curl_easy_cleanup(cp) };
}

fn new_multi() -> &'static Multi {
    unsafe { &*Multi::create() }
}

fn free(multi: &Multi) {
    unsafe { swr_v1_curl_multi_free(ptr::from_ref(multi).cast_mut()) };
}

fn raw(multi: &Multi) -> *mut Multi {
    ptr::from_ref(multi).cast_mut()
}

#[test]
fn poll_actions_map_to_reactor_events() {
    assert_eq!(events(CURL_POLL_IN), READ, "in");
    assert_eq!(events(CURL_POLL_OUT), WRITE, "out");
    assert_eq!(events(CURL_POLL_INOUT), READ | WRITE, "inout");
    assert_eq!(events(CURL_POLL_REMOVE), 0, "remove");
    assert_eq!(
        poll_events(READ | WRITE),
        libc::POLLIN | libc::POLLOUT,
        "poll"
    );
    assert_eq!(select_bits(libc::POLLHUP), CURL_CSELECT_IN, "hangup reads");
    assert_eq!(
        select_bits(libc::POLLNVAL | libc::POLLOUT),
        CURL_CSELECT_ERR | CURL_CSELECT_OUT,
        "error"
    );
}

#[test]
fn a_handle_survives_reset_and_is_destroyed() {
    let cp = easy("http://127.0.0.1/");
    let handle = unsafe { swr_v1_curl_get_handle(cp) };
    assert!(!handle.is_null(), "created");
    unsafe { swoole_curl_easy_reset(cp) };
    assert_eq!(
        unsafe { swr_v1_curl_get_handle(cp) },
        handle,
        "kept across reset"
    );
    close(cp);
}

#[test]
fn exec_without_a_coroutine_asks_cxx_for_the_fatal_error() {
    LOOP.with(|on| on.set(true));
    take();
    let cp = easy("http://127.0.0.1:1/");
    let handle = unsafe { swr_v1_curl_get_handle(cp) };
    let multi = new_multi();
    assert_eq!(
        swr_v1_curl_multi_exec(multi, unsafe { &mut *handle }),
        EXEC_NEEDS_COROUTINE,
        "needs one"
    );
    assert_eq!(unsafe { (*handle).multi }, raw(multi), "left in the multi");
    assert_eq!(
        take().first().map(String::as_str),
        Some("timer 1"),
        "curl's immediate timeout"
    );
    assert_eq!(
        multi.remove_handle(unsafe { &mut *handle }),
        CURLM_OK,
        "removed"
    );
    assert!(unsafe { (*handle).multi }.is_null(), "detached");
    free(multi);
    close(cp);
}

/// A transfer to a server that accepts and never answers, performed until
/// curl watches its socket.
struct Transfer {
    _listener: TcpListener,
    cp: *mut CURL,
    handle: *mut Handle,
    multi: &'static Multi,
}

fn transfer() -> Transfer {
    LOOP.with(|on| on.set(true));
    take();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cp = easy(&format!("http://{}/", listener.local_addr().unwrap()));
    let handle = unsafe { swr_v1_curl_get_handle(cp) };
    let multi = new_multi();
    assert_eq!(multi.add_handle(unsafe { &mut *handle }), CURLM_OK, "added");
    for _ in 0..100 {
        assert_eq!(multi.perform(), CURLM_OK, "performed");
        if !multi.sockets.borrow().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        multi.timer_callback.set(true);
    }
    assert_eq!(swr_v1_curl_multi_running(multi), 1, "running");
    assert_eq!(multi.sockets.borrow().len(), 1, "one socket");
    assert!(
        take().iter().any(|entry| entry.starts_with("new ")),
        "made a reactor socket"
    );
    Transfer {
        _listener: listener,
        cp,
        handle,
        multi,
    }
}

impl Transfer {
    fn finish(self) {
        assert_eq!(
            self.multi.remove_handle(unsafe { &mut *self.handle }),
            CURLM_OK,
            "removed"
        );
        assert!(
            take().contains(&"free".to_string()),
            "socket freed with the transfer"
        );
        assert!(self.multi.sockets.borrow().is_empty(), "no sockets");
        assert_eq!(self.multi.ready(), 0, "no active socket");
        free(self.multi);
        close(self.cp);
    }
}

#[test]
fn perform_registers_the_transfer_socket_and_removal_frees_it() {
    transfer().finish();
}

#[test]
fn a_waiterless_socket_event_leaves_the_reactor() {
    let transfer = transfer();
    let multi = transfer.multi;
    let socket = *multi.sockets.borrow().values().next().unwrap();
    unsafe { socket_event(socket.cast(), CURL_CSELECT_IN) };
    assert_eq!(take(), ["del"], "no waiter: out of the reactor");
    multi.selector_prepare();
    assert_eq!(take().len(), 1, "back in before waiting");
    multi.co.set(ptr::dangling_mut::<u8>().cast());
    unsafe { socket_event(socket.cast(), CURL_CSELECT_IN) };
    unsafe { socket_event(socket.cast(), CURL_CSELECT_OUT) };
    assert_eq!(take(), ["defer"], "one deferred wake-up");
    assert_eq!(multi.ready(), 1, "one active socket");
    assert_eq!(
        unsafe { (*socket).bitmask.get() } & (CURL_CSELECT_IN | CURL_CSELECT_OUT),
        CURL_CSELECT_IN | CURL_CSELECT_OUT,
        "both events"
    );
    multi.co.set(ptr::null_mut());
    assert!(!multi_deferred(multi), "not released");
    assert!(!multi.defer_callback.get(), "wake-up done");
    transfer.finish();
}

#[test]
fn a_multi_freed_with_a_queued_wake_up_is_freed_by_the_wake_up() {
    LOOP.with(|on| on.set(true));
    let multi = new_multi();
    assert_eq!(multi.on_timeout(0), 0, "a timer");
    multi.co.set(ptr::dangling_mut::<u8>().cast());
    multi.callback(None);
    multi.co.set(ptr::null_mut());
    assert_eq!(take(), ["timer 1", "defer"], "a queued wake-up");
    free(multi);
    assert!(multi.released.get(), "released, not freed");
    multi.timer_callback.set(false);
    multi.callback(None);
    assert!(
        !multi.timer_callback.get(),
        "a released multi ignores events"
    );
    assert!(take().is_empty(), "the timer stays until the wake-up");
    assert!(multi_deferred(multi), "the wake-up frees it");
    assert!(!multi.defer_callback.get(), "no wake-up queued");
    free(multi);
    assert_eq!(take(), ["timer del"], "freed");
}

#[test]
fn timeouts_without_an_event_loop_fail() {
    LOOP.with(|on| on.set(false));
    take();
    let multi = new_multi();
    assert_eq!(multi.on_timeout(0), -1, "no loop");
    LOOP.with(|on| on.set(true));
    assert_eq!(multi.on_timeout(0), 0, "a loop");
    assert_eq!(multi.on_timeout(-1), 0, "cancel");
    assert_eq!(
        take(),
        ["timer 1", "timer del"],
        "zero waits 1 ms; -1 deletes it"
    );
    assert_eq!(multi.on_timeout(-1), 0, "nothing to cancel");
    assert_eq!(
        take(),
        ["timer 1000"],
        "-1 without a timer polls in a second"
    );
    free(multi);
}
