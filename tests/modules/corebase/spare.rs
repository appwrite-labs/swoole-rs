//! The trusted reader adapter only exposes a successful, bounded initialized prefix.
use super::*;

#[test]
fn read_appends_and_clamps_datagram_lengths() {
    let mut room = [MaybeUninit::uninit(); 5];
    let (reported, filled) = lend_spare(&mut room, |out| {
        out.put(b"a").unwrap();
        assert_eq!(
            unsafe {
                out.read(|room| {
                    room[..2].write_copy_of_slice(b"bc");
                    Some(2)
                })
            },
            Some(2)
        );
        assert_eq!(out.written(), b"abc");
        let reported = unsafe {
            out.read(|room| {
                room.write_copy_of_slice(b"de");
                Some(99)
            })
        };
        assert_eq!(out.written(), b"abcde");
        reported
    });
    assert_eq!((reported, filled), (Some(99), 5));
}

#[test]
fn zero_and_failed_reads_leave_the_initialized_prefix_alone() {
    let mut room = [MaybeUninit::uninit(); 3];
    let ((), filled) = lend_spare(&mut room, |out| {
        out.put(b"a").unwrap();
        assert_eq!(unsafe { out.read(|_| Some(0)) }, Some(0));
        assert_eq!(
            unsafe {
                out.read(|room| {
                    room[0].write(b'x');
                    None
                })
            },
            None
        );
        assert_eq!(out.written(), b"a");
    });
    assert_eq!(filled, 1);
}

#[test]
fn vectored_read_counts_short_reads_across_empty_rooms() {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    let (read, mut write) = UnixStream::pair().unwrap();
    write.write_all(b"abcde").unwrap();
    let mut head = [MaybeUninit::uninit(); 2];
    let mut empty = [];
    let mut tail = [MaybeUninit::uninit(); 4];
    let (received, filled) = lend_spare_vectors(
        [
            head.as_mut_slice(),
            empty.as_mut_slice(),
            tail.as_mut_slice(),
        ],
        |out| unsafe {
            out.read(|rooms| {
                let descriptors: Vec<_> = rooms.iter().map(ReadVec::descriptor).collect();
                let received = libc::readv(
                    read.as_raw_fd(),
                    descriptors.as_ptr(),
                    c_int::try_from(descriptors.len()).unwrap(),
                );
                (received, usize::try_from(received).ok())
            })
        },
    );
    assert_eq!((received, filled), (5, 5));
    assert_eq!(unsafe { head.assume_init_ref() }, b"ab");
    assert_eq!(unsafe { tail[..3].assume_init_ref() }, b"cde");
}
