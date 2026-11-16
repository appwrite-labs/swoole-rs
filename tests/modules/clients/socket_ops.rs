use super::*;

/// The native socket-control table is compiled in this test binary too. Its
/// constructors, address writes, timeout options and connect modes run against
/// loopback sockets; the scripted tests above still exercise failure ordering.
#[test]
#[cfg_attr(miri, ignore = "requires TCP sockets and kernel timeout options")]
fn native_socket_control_connects_and_keeps_failed_assignment_atomic() {
    struct NativeClient(Handle);
    impl Drop for NativeClient {
        fn drop(&mut self) {
            unsafe { self.0.free() };
        }
    }
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = c_int::from(listener.local_addr().unwrap().port());
    for mode in [Some(ConnectMode::Raw), Some(ConnectMode::Sync), None] {
        let owner = NativeClient(create(SocketType::SW_SOCK_TCP as c_int, false).unwrap());
        let this = owner.0;
        let effects = this.get().socket_ops;
        let fd = this.socket().fd();
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK,
            0
        );
        assert!((effects.assign)(this, c"127.0.0.1", port, false));
        let before = this.record().server_addr;
        assert!(!(effects.assign)(
            this,
            c"invalid numeric address",
            port,
            false
        ));
        assert_eq!(log::last_error(), ERROR_BAD_HOST_ADDR);
        assert_eq!(this.record().server_addr.len, before.len);
        assert_eq!(
            this.record().server_addr.addr.v4().sin_port,
            before.addr.v4().sin_port
        );
        assert_eq!(
            this.record().server_addr.addr.v4().sin_addr.s_addr,
            before.addr.v4().sin_addr.s_addr
        );
        (effects.kernel_timeout)(this, 0.25);
        for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            let mut timeout: libc::timeval = swoole_rs_corebase::sys::zeroed();
            swoole_rs_corebase::sys::getsockopt(fd, libc::SOL_SOCKET, option, &mut timeout)
                .unwrap();
            assert_eq!((timeout.tv_sec, timeout.tv_usec), (0, 250_000));
        }
        this.set_timeout(1.0);
        if let Some(mode) = mode {
            assert!((effects.connect)(this, mode));
        } else {
            assert!(matches!(
                (effects.connect_async)(this),
                Connect::Ready | Connect::Wait
            ));
            assert!(this.socket().wait_event(1000, Events::WRITE));
        }
        let mut error = -1;
        assert!((effects.socket_error)(this, &mut error));
        assert_eq!(error, 0);
        let saved = this.get().edit_record(|record| record.socket.take());
        assert!(!(effects.event)(this, EventOp::Add, Events::READ));
        this.get().edit_record(|record| record.socket = saved);
    }
}

/// A manually driven thread timer keeps this test independent of signals and
/// wall-clock timing while exercising the native create/cancel effects.
#[test]
#[cfg_attr(miri, ignore = "requires a native socket and thread timer")]
fn native_connect_timer_is_cancelled_before_client_release() {
    struct Timer;
    impl Drop for Timer {
        fn drop(&mut self) {
            swoole_rs_timer::swoole_timer_free();
        }
    }
    struct NativeClient(Handle);
    impl Drop for NativeClient {
        fn drop(&mut self) {
            unsafe { self.0.free() };
        }
    }
    assert!(!swoole_rs_timer::swoole_timer_is_available());
    swoole_rs_timer::swoole_timer_create(true);
    let _timer = Timer;
    let client = NativeClient(create(SocketType::SW_SOCK_TCP as c_int, true).unwrap());
    let this = client.0;
    let held = (this.get().socket_ops.timer_add)(60.0, this).unwrap();
    this.get().timer.set(Some(held));
    assert_eq!(swoole_rs_timer::thread_timer_count(), 1);
    this.timer_clear();
    assert!(this.get().timer.get().is_none());
    assert_eq!(swoole_rs_timer::thread_timer_count(), 0);
    this.timer_clear();
    assert_eq!(swoole_rs_timer::thread_timer_count(), 0);
}
