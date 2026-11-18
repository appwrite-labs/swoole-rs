use super::*;
use std::io::Read;
use std::os::fd::IntoRawFd;
use std::os::unix::net::UnixStream;

#[test]
fn a_close_callback_observes_the_released_descriptor() {
    let (stream, mut peer) = UnixStream::pair().unwrap();
    peer.set_nonblocking(true).unwrap();
    let owned = over(stream.into_raw_fd());
    let socket = owned.co();
    let called = std::rc::Rc::new(Cell::new(false));
    let observed = called.clone();
    crate::socket_extra::set_close_hook(
        socket,
        Box::new(move |_| {
            assert!(matches!(socket.descriptor(), Descriptor::Closed));
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
            observed.set(true);
        }),
    );
    assert!(socket.close());
    assert!(called.get());
    assert_eq!(socket.sock_fd(), -1);
}

#[test]
fn mapped_close_leaves_a_reused_key_alone() {
    use crate::socket_map;
    let fd = 1_000_003;
    let replacement = unopened();
    let next = replacement.co();
    assert!(socket_map::insert(fd, unopened(), false).is_none());
    assert_eq!(
        socket_map::close(fd, |_| {
            socket_map::retire(fd);
            assert!(socket_map::insert(fd, replacement, false).is_none());
            true
        }),
        Some(true)
    );
    assert_eq!(socket_map::remove(fd).unwrap().co(), next);
}
