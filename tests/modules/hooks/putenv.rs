use super::*;

#[test]
fn replacements_keep_the_allocation_libc_accepted() {
    let mut table = Table::new();
    let mut accepted = std::ptr::null();
    for (text, succeeds) in [
        (b"key=1", false),
        (b"key=2", true),
        (b"key=3", false),
        (b"key=4", true),
    ] {
        let previous = table.get(b"key".as_slice()).map(|s| s.to_bytes().to_vec());
        assert_eq!(
            Assignment::parse(text)
                .unwrap()
                .store(&mut table, |next, unset| {
                    assert!(!unset);
                    if let Some(previous) = previous {
                        assert_eq!(unsafe { CStr::from_ptr(accepted) }.to_bytes(), previous);
                    }
                    if succeeds {
                        accepted = next.as_ptr();
                    }
                    succeeds
                }),
            succeeds
        );
        if accepted.is_null() {
            assert!(table.is_empty());
        } else {
            let stored = &table[b"key".as_slice()];
            assert_eq!(stored.as_ptr(), accepted);
            assert_eq!(unsafe { CStr::from_ptr(accepted) }, stored.as_ref());
        }
    }
}

#[test]
fn parses_c_strings_and_unsets_by_name() {
    assert!(Assignment::parse(b"").is_none());
    assert!(Assignment::parse(b"=value").is_none());
    let mut table = Table::new();
    for (input, key, value, unset) in [
        (&b"TZ=a=b\0extra"[..], &b"TZ"[..], c"TZ=a=b", false),
        (b"TZ", b"TZ", c"TZ", true),
        (b"TZ\0=value", b"TZ\0=value", c"TZ", true),
    ] {
        let assignment = Assignment::parse(input).unwrap();
        assert_eq!(assignment.key, key);
        assert!(assignment.store(&mut table, |text, bare| {
            assert_eq!((text, bare), (value, unset));
            true
        }));
        assert_eq!(table[key].as_ref(), value);
    }
}
