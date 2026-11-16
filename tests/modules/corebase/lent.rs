//! Lent output counts stay within both the allocation and the syscall's count type,
//! and a lent address sends where its bytes say.
use super::*;

#[test]
fn limited_output_counts_never_grow_or_wrap() {
    let mut slots = [0u8; 8];
    for (limit, expected) in [(0, 0), (3, 3), (8, 8), (9, 8), (usize::MAX, 8)] {
        lend_out(&mut slots, |_, count: Count<'_, c_int>| {
            assert_eq!(count.at_most(limit).0, expected);
        });
        lend_out(&mut slots, |_, count: Count<'_>| {
            assert_eq!(count.at_most(limit).0, usize::try_from(expected).unwrap());
        });
    }
    let empty: &mut [u8] = &mut [];
    lend_out(empty, |_, count: Count<'_, c_int>| {
        assert_eq!(count.at_most(usize::MAX).0, 0);
    });
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no sockets")]
fn an_address_from_bytes_sends_where_its_bytes_say() {
    use std::os::fd::AsRawFd;
    let receiver = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut v4: libc::sockaddr_in = crate::sys::zeroed();
    #[cfg(target_vendor = "apple")]
    {
        v4.sin_len = u8::try_from(size_of_val(&v4)).unwrap();
    }
    v4.sin_family = libc::sa_family_t::try_from(libc::AF_INET).unwrap();
    v4.sin_port = receiver.local_addr().unwrap().port().to_be();
    v4.sin_addr.s_addr = u32::from(std::net::Ipv4Addr::LOCALHOST).to_be();
    let bytes =
        unsafe { std::slice::from_raw_parts((&raw const v4).cast::<u8>(), size_of_val(&v4)) };
    let address = SocketAddress::from_bytes(bytes);
    assert_eq!(address.len, crate::sys::socklen::<libc::sockaddr_in>());
    let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    assert_eq!(
        Drain::new(b"ping").sendto(sender.as_raw_fd(), 0, &address),
        Some(4)
    );
    let mut received = [0; 8];
    assert_eq!(receiver.recv(&mut received).unwrap(), 4);
    assert_eq!(&received[..4], b"ping");
    let long = SocketAddress::from_bytes(&[1; 200]);
    assert_eq!(long.len, crate::sys::socklen::<libc::sockaddr_un>());
}
