//! The former `gate.cc` and `callback_property.cc`, check for check: crates/ports' event
//! catalog against the pinned one and the pinned `getCallback` body
//! (`swoole_rs_oracle::ports`), over the same names. Each C export the gates called is
//! replaced by the safe function that was its body (README.md maps them).
use std::ffi::c_char;
use std::sync::Mutex;

use swoole_rs_oracle::ports as reference;
use swoole_rs_ports::{callback_property, event};

/// The process locale is global, and the pinned lookup folds case in it.
static LOCALE: Mutex<()> = Mutex::new(());

/// A pinned catalog entry: the event's key and its property (`on` and its name).
struct Entry {
    key: Vec<u8>,
    property: Vec<u8>,
}

/// `length` bytes at `data`, which the oracle returned.
fn bytes(data: *const c_char, length: usize) -> Vec<u8> {
    // SAFETY: the oracle's string, live until its next call.
    unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) }.to_vec()
}

/// The pinned catalog, in the order that numbers the getter model's properties from 1.
fn catalog() -> Vec<Entry> {
    (0..reference::events())
        .map(|index| {
            let (mut key, mut key_length) = (std::ptr::null(), 0);
            let (mut name, mut name_length) = (std::ptr::null(), 0);
            // SAFETY: `index` is in range; the outputs are live locals.
            unsafe {
                reference::entry(
                    index,
                    &raw mut key,
                    &raw mut key_length,
                    &raw mut name,
                    &raw mut name_length,
                )
            };
            Entry {
                key: bytes(key, key_length),
                property: [b"on", &bytes(name, name_length)[..]].concat(),
            }
        })
        .collect()
}

/// The pinned catalog's callback type and property for `name`.
fn pinned_event(name: &[u8], folded: bool) -> Option<(usize, Vec<u8>)> {
    let (mut found, mut found_length) = (std::ptr::null(), 0);
    // SAFETY: `name` is live for its length; the outputs are live locals.
    let index = unsafe {
        reference::event(
            name.as_ptr().cast(),
            name.len(),
            folded,
            &raw mut found,
            &raw mut found_length,
        )
    };
    // -1 for no event.
    let index = usize::try_from(index).ok()?;
    Some((index, [b"on", &bytes(found, found_length)[..]].concat()))
}

/// `gate.cc`'s `event`: two checks.
fn check_event(name: &[u8], folded: bool, checks: &mut u64) {
    let expected = pinned_event(name, folded);
    let actual = event(name, folded);
    assert_eq!(
        actual.map(|(index, _)| index),
        expected.as_ref().map(|&(index, _)| index),
        "{name:?} folded={folded}"
    );
    assert_eq!(
        actual.map(|(_, property)| property),
        expected.as_ref().map(|(_, property)| &property[..]),
        "{name:?} folded={folded}"
    );
    *checks += 2;
}

fn upper(key: &[u8]) -> Vec<u8> {
    key.to_ascii_uppercase()
}

/// `gate.cc`: every key, its upper case and every byte at every position, matched
/// exactly and as `on()` folds, against the pinned catalog.
#[test]
fn event_catalog() {
    let _guard = LOCALE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut checks = 0;
    for entry in catalog() {
        check_event(&entry.key, false, &mut checks);
        check_event(&entry.key, true, &mut checks);
        let upper = upper(&entry.key);
        check_event(&upper, true, &mut checks);
        check_event(&upper, false, &mut checks);
        for position in 0..entry.key.len() {
            for byte in 0..=255u8 {
                let mut changed = entry.key.clone();
                changed[position] = byte;
                check_event(&changed, true, &mut checks);
                check_event(&changed, false, &mut checks);
            }
        }
    }
    check_event(b"", true, &mut checks);
    check_event(b"unknown", false, &mut checks);
    check_event(b"connect\0", true, &mut checks);
    // The gate's null span of no bytes: an empty name.
    assert_eq!(event(&[], true), None);
    checks += 1;
    assert_eq!(checks, 108_647);
}

/// The Rust getter (`crates/phpserver/src/port.rs`'s `get_callback`) in the witness's
/// terms: the name's string conversion, the Rust lowering, a silent property read and
/// a copy of a property that isn't null. Its Zend calls in order, and the number of
/// the property it returned (0 for null).
fn rust_get_callback(name: &[u8], properties: &[Entry], null_properties: bool) -> (Vec<u8>, u32) {
    let mut calls = b"convert;".to_vec();
    if let Some(property) = callback_property(name) {
        calls.extend_from_slice(b"read:");
        calls.extend_from_slice(property);
        calls.push(b';');
        let found = (1u32..)
            .zip(properties)
            .find(|(_, entry)| entry.property == property)
            .filter(|_| !null_properties);
        if let Some((identity, _)) = found {
            calls.extend_from_slice(format!("copy:{identity};").as_bytes());
            return (calls, identity);
        }
    }
    calls.extend_from_slice(b"null;");
    (calls, 0)
}

/// The pinned getter over the oracle's model: as [`rust_get_callback`].
fn pinned_get_callback(name: &[u8]) -> (Vec<u8>, u32) {
    let (mut calls, mut calls_length, mut lowercased) = (std::ptr::null(), 0, 0);
    // SAFETY: `name` is live for its length; the outputs are live locals.
    let identity = unsafe {
        reference::get_callback(
            name.as_ptr().cast(),
            name.len(),
            &raw mut calls,
            &raw mut calls_length,
            &raw mut lowercased,
        )
    };
    assert_eq!(lowercased, 1, "the pinned getter lowercases once");
    (bytes(calls, calls_length), identity)
}

/// `callback_property.cc`: the pinned `getCallback` body against the Rust getter over
/// the keys, their upper case with every byte at every position, and 4096 random names,
/// with every property set and with every property null, in each locale the host has.
#[test]
#[expect(
    clippy::print_stderr,
    reason = "an unavailable locale is reported, as the witness skipped it"
)]
fn get_callback() {
    let _guard = LOCALE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let catalog = catalog();
    let mut names: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"unknown".to_vec(),
        b"connect\0".to_vec(),
        b"receive-extra".to_vec(),
    ];
    for entry in &catalog {
        names.push(entry.key.clone());
        let upper = upper(&entry.key);
        names.push(upper.clone());
        for position in 0..entry.key.len() {
            for byte in 0..=255u8 {
                let mut name = upper.clone();
                name[position] = byte;
                names.push(name);
            }
        }
    }
    let mut seed: u32 = 0x81715;
    let mut random_word = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for _ in 0..4096 {
        let length = random_word() % 64;
        names.push(
            (0..length)
                .map(|_| random_word().to_le_bytes()[0])
                .collect(),
        );
    }

    let (mut cases, mut locales) = (0u64, 0u64);
    for locale in [c"C", c"", c"tr_TR.UTF-8"] {
        // SAFETY: a NUL-terminated name; the lock keeps other tests off the locale.
        if unsafe { libc::setlocale(libc::LC_CTYPE, locale.as_ptr()) }.is_null() {
            eprintln!("locale {locale:?}: unsupported");
            continue;
        }
        locales += 1;
        for null_properties in [false, true] {
            reference::properties(null_properties);
            for name in &names {
                assert_eq!(
                    rust_get_callback(name, &catalog, null_properties),
                    pinned_get_callback(name),
                    "case {cases}: {name:?}"
                );
                cases += 1;
            }
        }
    }
    // SAFETY: as above.
    unsafe { libc::setlocale(libc::LC_CTYPE, c"C".as_ptr()) };
    assert!(locales >= 1, "no locale was available");
    // 187,560 with all three locales.
    assert_eq!(cases, 62_520 * locales);
}
