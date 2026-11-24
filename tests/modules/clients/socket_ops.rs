use super::*;

#[test]
fn native_receive_preserves_initialized_tails_and_fills_spare_capacity() {
    use std::io::Write;
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let owner = create(SocketType::SW_SOCK_TCP as c_int, false).unwrap();
    let this = owner.handle();
    let effects = this.get().socket_ops;
    assert!((effects.assign)(
        this,
        c"127.0.0.1",
        c_int::from(listener.local_addr().unwrap().port()),
        false
    ));
    assert!((effects.connect)(this, ConnectMode::Raw));
    let (mut peer, _) = listener.accept().unwrap();
    peer.write_all(b"A\0BC\0D").unwrap();
    let mut initialized = [0xa5; 6];
    assert_eq!(
        recv(this, &mut initialized[..3], libc::MSG_WAITALL),
        Some(3)
    );
    assert_eq!(initialized, [b'A', 0, b'B', 0xa5, 0xa5, 0xa5]);
    let mut spare = [core::mem::MaybeUninit::uninit(); 3];
    assert_eq!(recv(this, &mut spare, libc::MSG_WAITALL), Some(3));
    assert_eq!(spare.map(|byte| unsafe { byte.assume_init() }), *b"C\0D");
    drop(peer);
    assert_eq!(recv(this, &mut initialized, 0), Some(0));
}

#[test]
#[cfg_attr(miri, ignore = "requires UDP sockets")]
fn native_async_datagram_receive_records_the_sender_for_both_buffer_kinds() {
    let peer = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let owner = create(SocketType::SW_SOCK_UDP as c_int, true).unwrap();
    let this = owner.handle();
    assert!(this.bind(c"127.0.0.1", 0));
    assert!(
        this.socket()
            .edit(swoole_rs_corebase::reactor::SocketFields::get_name)
    );
    let port = this
        .socket()
        .read(swoole_rs_corebase::reactor::SocketFields::port);
    peer.send_to(b"A\0B", (std::net::Ipv4Addr::LOCALHOST, port))
        .unwrap();
    let mut initialized = [0xa5; 6];
    assert_eq!(recv(this, &mut initialized, 0), Some(3));
    assert_eq!(initialized, [b'A', 0, b'B', 0xa5, 0xa5, 0xa5]);
    let sender = |this: Handle<'_>| u16::from_be(this.record().remote_addr.addr.v4().sin_port);
    assert_eq!(sender(this), peer.local_addr().unwrap().port());
    peer.send_to(b"C\0D", (std::net::Ipv4Addr::LOCALHOST, port))
        .unwrap();
    let mut spare = [core::mem::MaybeUninit::uninit(); 3];
    assert_eq!(recv(this, &mut spare, 0), Some(3));
    assert_eq!(spare.map(|byte| unsafe { byte.assume_init() }), *b"C\0D");
    assert_eq!(sender(this), peer.local_addr().unwrap().port());
}

/// The native socket-control table is compiled in this test binary too. Its
/// constructors, address writes, timeout options and connect modes run against
/// loopback sockets; the scripted tests above still exercise failure ordering.
#[test]
#[cfg_attr(miri, ignore = "Miri has no blocking connect with SO_SNDTIMEO set")]
fn native_socket_control_connects_and_keeps_failed_assignment_atomic() {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = c_int::from(listener.local_addr().unwrap().port());
    for mode in [Some(ConnectMode::Raw), Some(ConnectMode::Sync), None] {
        let owner = create(SocketType::SW_SOCK_TCP as c_int, false).unwrap();
        let this = owner.handle();
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
        // This thread has no reactor.
        assert!(!(effects.event)(this, EventOp::Add, Events::READ));
    }
}

/// A manually driven thread timer keeps this test independent of signals and
/// wall-clock timing while exercising the native create/cancel effects.
#[test]
fn native_connect_timer_is_cancelled_before_client_release() {
    struct Timer;
    impl Drop for Timer {
        fn drop(&mut self) {
            swoole_rs_corebase::timer::swoole_timer_free();
        }
    }
    assert!(!swoole_rs_corebase::timer::swoole_timer_is_available());
    swoole_rs_corebase::timer::swoole_timer_create(true);
    let _timer = Timer;
    let client = create(SocketType::SW_SOCK_TCP as c_int, true).unwrap();
    let this = client.handle();
    let held = (this.get().socket_ops.timer_add)(60.0, this).unwrap();
    this.get().timer.set(Some(held));
    assert_eq!(swoole_rs_corebase::timer::thread_timer_count(), 1);
    this.timer_clear();
    assert!(this.get().timer.get().is_none());
    assert_eq!(swoole_rs_corebase::timer::thread_timer_count(), 0);
    this.timer_clear();
    assert_eq!(swoole_rs_corebase::timer::thread_timer_count(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "requires TCP sockets and OpenSSL")]
fn native_tls_effects_keep_the_session_and_settings_while_the_peer_is_pending() {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let owner = create(SocketType::SW_SOCK_TCP as c_int, false).unwrap();
    let this = owner.handle();
    let effects = this.get().socket_ops;
    assert!(!(effects.tls_connect)(this.socket()));
    assert!(!(effects.tls_verify)(this.socket(), false));
    assert!(!(effects.tls_check_host)(this.socket(), c"first.test"));
    assert!((effects.assign)(
        this,
        c"127.0.0.1",
        c_int::from(listener.local_addr().unwrap().port()),
        false
    ));
    assert!((effects.connect)(this, ConnectMode::Raw));
    let (_peer, _) = listener.accept().unwrap();
    assert!(
        this.socket()
            .edit(|record| { record.set_fd_option_or_warn(1, -1) })
    );
    assert!(enable_ssl(this).is_some());
    let context = this.get().tls_context().unwrap().clone();
    assert!(
        context
            .borrow_configuration()
            .set_text(swoole_rs_corebase::tls::Text::HostName, b"first.test")
    );
    assert!(ssl_handshake(this));
    let session = this
        .socket()
        .read(swoole_rs_corebase::reactor::SocketFields::ssl);
    assert!(!session.is_null());
    assert!(this.tls_awaits_stream());
    assert!(
        this.socket()
            .read(swoole_rs_corebase::reactor::SocketFields::tls_wants)
            .contains(Events::READ)
    );
    assert_eq!(
        this.get().tls_settings.get().unwrap().host_name(),
        c"first.test"
    );
    assert!(
        context
            .borrow_configuration()
            .set_text(swoole_rs_corebase::tls::Text::HostName, b"later.test")
    );
    assert!(ssl_handshake(this));
    assert_eq!(
        this.socket()
            .read(swoole_rs_corebase::reactor::SocketFields::ssl),
        session
    );
    assert_eq!(
        this.get().tls_settings.get().unwrap().host_name(),
        c"first.test"
    );
    assert!(this.tls_awaits_stream());
    assert!(close(this));
    assert!(this.socket().read(|record| record.ssl().is_null()));
    assert!(!close(this));
}
