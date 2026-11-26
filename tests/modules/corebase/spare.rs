//! Receives count only a successful, bounded written prefix.
use super::*;

/// A ring that writes the bytes it holds and reports `reported`.
struct Script<'a> {
    bytes: &'a [u8],
    reported: Option<usize>,
}

// SAFETY: test stand-in; each case below writes at least the prefix it reports, or
// reports more only when it filled the whole room.
unsafe impl Ring for Script<'_> {
    fn recv(&mut self, room: &mut [MaybeUninit<u8>]) -> Option<usize> {
        let n = self.bytes.len().min(room.len());
        room[..n].write_copy_of_slice(&self.bytes[..n]);
        self.reported
    }
    fn readv(&mut self, _: &mut [ReadVec<'_>]) -> Option<usize> {
        unreachable!("a scalar receive")
    }
    fn writev(&mut self, _: &[IoSlice<'_>]) -> Option<usize> {
        unreachable!("a receive")
    }
}

#[test]
fn read_appends_and_clamps_datagram_lengths() {
    let mut room = [MaybeUninit::uninit(); 5];
    let (reported, filled) = lend_spare(&mut room, |out| {
        out.put(b"a").unwrap();
        let mut bc = Script {
            bytes: b"bc",
            reported: Some(2),
        };
        assert_eq!(out.receive(&mut bc), Some(2));
        assert_eq!(out.written(), b"abc");
        let reported = out.receive(&mut Script {
            bytes: b"de",
            reported: Some(99),
        });
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
        let zero = Script {
            bytes: b"",
            reported: Some(0),
        };
        assert_eq!(out.receive(&mut { zero }), Some(0));
        let failed = Script {
            bytes: b"x",
            reported: None,
        };
        assert_eq!(out.receive(&mut { failed }), None);
        out.uncounted(|room| {
            room[0].write(b'y');
        });
        assert_eq!(out.written(), b"a");
    });
    assert_eq!(filled, 1);
}

#[test]
fn read_vector_lends_only_the_remaining_stream_buffer() {
    let mut bytes = *b"abc";
    let mut room = ReadVec::new(&mut bytes);
    room.advance(1);
    room.fill().slots().write_copy_of_slice(b"de");
    room.advance(2);
    assert!(room.fill().slots().is_empty());
    assert_eq!(&bytes, b"ade");
}
