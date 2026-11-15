//! The former `locale_gate.cc`, check for check: crates/http's case-insensitive helpers
//! against the pinned bodies (`swoole_rs_oracle::http_locale`), which fold case through
//! libc's `strncasecmp`, in each locale this host has. Each C export the gate called is
//! replaced by the safe function that was its body (README.md maps them).
use std::ffi::{c_char, c_int};
use std::sync::Mutex;

use swoole_rs_http::{
    HttpStaticPath, boundary, location_match, method, method_string, multipart_disposition,
    multipart_filename,
};
use swoole_rs_oracle::http_locale as reference;

/// The process locale is global: one test at a time.
static LOCALE: Mutex<()> = Mutex::new(());

/// Run `check` under each of the gate's locales that `setlocale` accepts. A locale the
/// host lacks is reported, not passed; at least one must be available, as the gate required.
#[expect(
    clippy::print_stderr,
    reason = "an unavailable locale is reported, as the gate reported it unsupported"
)]
fn in_each_locale(mut check: impl FnMut()) {
    let _guard = LOCALE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut available = 0;
    for locale in [
        c"C",
        c"C.UTF-8",
        c"en_US.ISO8859-1",
        c"tr_TR.ISO8859-9",
        c"tr_TR.UTF-8",
    ] {
        // SAFETY: a NUL-terminated name; the lock keeps other tests off the locale.
        if unsafe { libc::setlocale(libc::LC_ALL, locale.as_ptr()) }.is_null() {
            eprintln!("locale {locale:?}: unsupported");
            continue;
        }
        available += 1;
        check();
    }
    // SAFETY: as above.
    unsafe { libc::setlocale(libc::LC_ALL, c"C".as_ptr()) };
    assert!(available > 0, "no locale was available");
}

/// libc's `tolower` or `toupper` of a byte in the current locale, as `char(std::tolower(..))`.
fn fold(byte: u8, to: unsafe extern "C" fn(c_int) -> c_int) -> u8 {
    // SAFETY: an `unsigned char` value, which both functions take.
    unsafe { to(c_int::from(byte)) }.to_le_bytes()[0]
}

fn strcaseeq(a: &[u8], b: &[u8]) -> bool {
    // SAFETY: both slices are live for their lengths.
    unsafe { reference::strcaseeq(a.as_ptr().cast(), a.len(), b.as_ptr().cast(), b.len()) != 0 }
}

/// The pinned inline location helper, including every high-byte pair and NUL.
#[test]
fn location() {
    in_each_locale(|| {
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                let url = [b"/", &[a][..], b"/file"].concat();
                let location = [b'/', b];
                // SAFETY: both slices are live for their lengths.
                let old = unsafe {
                    reference::istarts_with(
                        url.as_ptr().cast(),
                        url.len(),
                        location.as_ptr().cast(),
                        location.len(),
                    )
                } != 0;
                let mut state = HttpStaticPath {
                    phase: swoole_rs_http::StaticPhase::Locations,
                    ..HttpStaticPath::default()
                };
                let now = location_match(&url, &location, &mut state);
                assert!(
                    now == Ok(old) && u32::from(state.last) == u32::from(old),
                    "location:{a}:{b}"
                );
            }
        }
    });
}

#[test]
fn methods() {
    in_each_locale(|| {
        for id in 1..=26 {
            let name = method_string(id).unwrap().to_bytes();
            for mask in 0..256u32 {
                let mut value = name.to_vec();
                for (i, byte) in value.iter_mut().enumerate() {
                    if mask & (1 << (i % 8)) != 0 {
                        *byte = fold(*byte, libc::tolower);
                    }
                }
                // SAFETY: `value` is live for its length.
                let old =
                    unsafe { reference::get_method(value.as_ptr().cast::<c_char>(), value.len()) };
                assert_eq!(old, method(&value), "method:{id}:{mask}");
            }
        }
    });
}

#[test]
fn filename() {
    in_each_locale(|| {
        for mask in 0..256u32 {
            let mut key = *b"filename";
            for (i, byte) in key.iter_mut().enumerate() {
                if mask & (1 << i) != 0 {
                    *byte = fold(*byte, libc::toupper);
                }
            }
            let expected = strcaseeq(&key, b"filename");
            let mut scratch = [&key[..], b"=x\0"].concat();
            // The export lent the scratch through its first NUL.
            let end = scratch.iter().position(|&byte| byte == 0).unwrap();
            assert_eq!(
                multipart_filename(&mut scratch[..=end]),
                Ok(expected),
                "filename:{mask}"
            );
        }
    });
}

#[test]
fn boundaries() {
    let name = b"boundary=";
    in_each_locale(|| {
        for i in 0..name.len() {
            for byte in 0..=255u8 {
                let mut value = name.to_vec();
                value[i] = byte;
                value.extend_from_slice(b"payload");
                let (mut offset, mut length): (usize, c_int) = (0, -1);
                // SAFETY: `value` is live for its length; both outputs are locals.
                let found = unsafe {
                    reference::boundary(
                        value.as_ptr().cast(),
                        value.len(),
                        0,
                        &raw mut offset,
                        &raw mut length,
                    )
                };
                let expected = found.then(|| (offset, usize::try_from(length).unwrap()));
                let span = boundary(&value, 0).map(|span| span.map(|s| (s.offset, s.length)));
                assert_eq!(span, Ok(expected), "boundary:{i}:{byte}");
            }
        }
    });
}

#[test]
fn disposition() {
    let name = b"content-disposition";
    in_each_locale(|| {
        for i in 0..name.len() {
            for byte in 0..=255u8 {
                let mut value = name.to_vec();
                value[i] = byte;
                let expected = strcaseeq(&value, name);
                assert_eq!(
                    multipart_disposition(&value),
                    expected,
                    "disposition:{i}:{byte}"
                );
            }
        }
    });
}
