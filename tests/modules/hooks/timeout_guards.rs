use super::TimeoutScope;
use crate::co_socket::TestSocket;
use swoole_rs_corebase::reactor::TimeoutKind;

#[test]
fn timeout_guards_restore_all_masks_after_exception() {
    let mut owner = TestSocket::new();
    let sock = owner.co();
    let originals = [0.125, 0.25, 0.5, 1.0];
    for (bit, original) in TimeoutKind::EACH.into_iter().zip(originals) {
        sock.set_timeout(original, bit);
    }
    for mask in 0..16 {
        let mask = TimeoutKind::from_bits(mask);
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = TimeoutScope::new(sock, 2.0, mask);
            for (bit, original) in TimeoutKind::EACH.into_iter().zip(originals) {
                assert_eq!(
                    sock.timeout(bit),
                    if mask.intersects(bit) { 2.0 } else { original }
                );
            }
            std::panic::resume_unwind(Box::new("unwind timeout guard"));
        }));
        assert_eq!(
            unwind.unwrap_err().downcast_ref::<&str>(),
            Some(&"unwind timeout guard")
        );
        for (bit, original) in TimeoutKind::EACH.into_iter().zip(originals) {
            assert_eq!(sock.timeout(bit), original);
        }
    }
}

#[test]
fn nested_timeout_guards_restore_outer_values() {
    let mut owner = TestSocket::new();
    let sock = owner.co();
    sock.set_timeout(0.125, TimeoutKind::ALL);
    {
        let _outer = TimeoutScope::new(sock, 1.0, TimeoutKind::ALL);
        {
            let _inner = TimeoutScope::new(sock, 2.0, TimeoutKind::RDWR);
            for (bit, expected) in TimeoutKind::EACH.into_iter().zip([1.0, 1.0, 2.0, 2.0]) {
                assert_eq!(sock.timeout(bit), expected);
            }
        }
        let _unchanged = TimeoutScope::new(sock, 0.0, TimeoutKind::ALL);
        for bit in TimeoutKind::EACH {
            assert_eq!(sock.timeout(bit), 1.0);
        }
    }
    for bit in TimeoutKind::EACH {
        assert_eq!(sock.timeout(bit), 0.125);
    }
}
