//! The former `corebase-native` gate (README.md maps each part): `cases.py`'s corpus
//! through crates/corebase's number helpers and through the pinned base.cc bodies
//! (`swoole_rs_oracle::corebase`), each case from its preset errno, with the result and
//! the errno after it compared and the independently computed value checked; the seven
//! `TEST(base, ...)` bodies the gate extracted from upstream's `base.cpp`; and
//! `allocator.cc`'s `strndup`, which this binary defines, so both sides' `swoole_dec2hex`
//! allocate through it.
use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_int, c_long, c_ulong};
use std::sync::{Mutex, MutexGuard, PoisonError};

use swoole_rs_corebase::sys::{errno, set_errno};
use swoole_rs_corebase::{
    get_env, parse_stoi, swoole_common_divisor, swoole_common_multiple, swoole_dec2hex,
    swoole_hex2dec, swoole_itoa, swoole_type_size, swoole_version_compare,
};
use swoole_rs_oracle::corebase as pinned;

/// The locale and the environment are the process's: one test at a time.
static PROCESS: Mutex<()> = Mutex::new(());

fn exclusive() -> MutexGuard<'static, ()> {
    PROCESS.lock().unwrap_or_else(PoisonError::into_inner)
}

thread_local! {
    /// `allocator.cc`'s `allocations`, `requested` and `fail_allocation`.
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static REQUESTED: Cell<usize> = const { Cell::new(0) };
    static FAIL_ALLOCATION: Cell<bool> = const { Cell::new(false) };
    /// Set while the allocator test runs, in the C locale: `strndup` then calls back
    /// into a number helper, as the fixture's did.
    static REENTER: Cell<bool> = const { Cell::new(false) };
}

/// `allocator.cc`'s standard `strndup` interposer: counts, can fail with `ENOMEM`,
/// and otherwise copies as libc's does.
#[unsafe(no_mangle)]
unsafe extern "C" fn strndup(input: *const c_char, maximum: usize) -> *mut c_char {
    _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
    _ = REQUESTED.try_with(|requested| requested.set(maximum));
    if REENTER.try_with(Cell::get).unwrap_or(false) {
        // A real native allocator may call back into numeric helpers without a borrow.
        assert_eq!(hex2dec(c"FFtail"), (16_833_333, 6), "reentrant helper");
    }
    if FAIL_ALLOCATION.try_with(Cell::get).unwrap_or(false) {
        set_errno(libc::ENOMEM);
        return std::ptr::null_mut();
    }
    // SAFETY: the caller's string, read to its NUL or `maximum` bytes.
    let length = unsafe { libc::strnlen(input, maximum) };
    // SAFETY: a fresh allocation, or null.
    let result = unsafe { libc::malloc(length + 1) }.cast::<c_char>();
    if !result.is_null() {
        // SAFETY: `length` readable bytes into `length + 1` fresh ones.
        unsafe {
            result.copy_from_nonoverlapping(input, length);
            result.add(length).write(0);
        }
    }
    result
}

/// Each helper as crates/corebase has it and as the pin has it, over the same types.
struct Engine {
    name: &'static str,
    type_size: fn(c_char) -> c_int,
    dec2hex: fn(c_ulong, c_int) -> *mut c_char,
    hex2dec: fn(&CStr) -> (c_ulong, usize),
    version_compare: fn(&CStr, &CStr) -> c_int,
    common_divisor: fn(u32, u32) -> u32,
    common_multiple: fn(u32, u32) -> u32,
    /// The count, the digits and the byte after them.
    itoa: fn(c_long) -> (c_int, Vec<u8>, u8),
}

fn hex2dec(text: &CStr) -> (c_ulong, usize) {
    // The driver's `size_t parsed = 123`.
    let mut parsed = 123;
    // SAFETY: the corpus keeps to bytes `toupper` takes in this target's `char` mode.
    let value = unsafe { swoole_hex2dec(text.into(), &mut parsed) };
    (value, parsed)
}

/// What `swoole_itoa` left in the driver's `char buf[128]`.
fn decimal(count: c_int, buffer: &[u8]) -> (c_int, Vec<u8>, u8) {
    let length = usize::try_from(count).unwrap_or(usize::MAX);
    let digits = buffer.get(..length).unwrap_or_default().to_vec();
    (count, digits, buffer.get(length).copied().unwrap_or(0xff))
}

const RUST: Engine = Engine {
    name: "rust",
    type_size: |kind| swoole_type_size(kind),
    dec2hex: |value, base| swoole_dec2hex(value, base),
    hex2dec,
    version_compare: |left, right| swoole_version_compare(left.into(), right.into()),
    common_divisor: |u, v| swoole_common_divisor(u, v),
    common_multiple: |u, v| swoole_common_multiple(u, v),
    itoa: |value| {
        let mut buffer = [0xa5_u8; 128];
        let count = swoole_itoa(&mut buffer, value);
        decimal(count, &buffer)
    },
};

const PINNED: Engine = Engine {
    name: "pinned",
    type_size: |kind| pinned::type_size(kind),
    // SAFETY: the corpus keeps `base` within 2 to 36.
    dec2hex: |value, base| unsafe { pinned::dec2hex(value, base) },
    hex2dec: |text| {
        let mut parsed = 123;
        // SAFETY: a NUL-terminated string and a live output.
        let value = unsafe { pinned::hex2dec(text.as_ptr(), &raw mut parsed) };
        (value, parsed)
    },
    // SAFETY: two NUL-terminated strings.
    version_compare: |left, right| unsafe {
        pinned::version_compare(left.as_ptr(), right.as_ptr())
    },
    // SAFETY: the corpus has no zero operand.
    common_divisor: |u, v| unsafe { pinned::common_divisor(u, v) },
    // SAFETY: as above.
    common_multiple: |u, v| unsafe { pinned::common_multiple(u, v) },
    itoa: |value| {
        let mut buffer = [0xa5_u8; 128];
        // SAFETY: 128 bytes hold any `long`'s digits and their NUL.
        let count = unsafe { pinned::itoa(buffer.as_mut_ptr().cast(), value) };
        decimal(count, &buffer)
    },
};

/// One case as the driver ran it: errno preset, the call, then errno read back.
fn observed<T>(initial: c_int, call: impl FnOnce() -> T) -> (T, c_int) {
    set_errno(initial);
    let value = call();
    (value, errno())
}

/// The driver's `setlocale(LC_ALL, name)`: false for a locale this host lacks.
fn set_locale(name: &CStr) -> bool {
    // SAFETY: a NUL-terminated name; `PROCESS` keeps other tests off the locale.
    !unsafe { libc::setlocale(libc::LC_ALL, name.as_ptr()) }.is_null()
}

/// The driver's `decode(a).c_str()`: the bytes before the first NUL.
fn text(bytes: &[u8]) -> CString {
    CString::new(bytes.split(|byte| *byte == 0).next().unwrap_or_default()).unwrap_or_default()
}

/// `swoole_dec2hex`'s digits, freed.
fn digits(pointer: *mut c_char) -> Vec<u8> {
    assert!(!pointer.is_null(), "swoole_dec2hex allocated");
    // SAFETY: a NUL-terminated `strndup` allocation.
    let digits = unsafe { CStr::from_ptr(pointer) }.to_bytes().to_vec();
    // SAFETY: the allocation, which nothing else holds.
    unsafe { libc::free(pointer.cast()) };
    digits
}

fn gcd(mut u: u64, mut v: u64) -> u64 {
    while v != 0 {
        (u, v) = (v, u % v);
    }
    u
}

const PAIRS: [(u32, u32); 9] = [
    (1, 1),
    (1, 257),
    (257, 1),
    (65535, 65536),
    (65536, 65535),
    (2_147_483_647, 2_147_483_646),
    (4_294_967_295, 4_294_967_295),
    (4_294_967_295, 4_294_967_294),
    (1_836_311_903, 1_134_903_170),
];
const LOCALES: [&CStr; 5] = [c"C", c"POSIX", c"C.UTF-8", c"en_US.UTF-8", c"tr_TR.UTF-8"];
/// The compiler's plain `char`, as `--signed-char` and `--unsigned-char` chose it for
/// the gate: the pin is compiled, and crates/corebase promotes, in the target's mode.
const UNSIGNED_CHAR: bool = c_char::MIN == 0;

/// `cases.py`'s hex inputs: the fixed ones, then every single byte this `char` mode
/// may hand `toupper` (1 to 127, and through 255 where `char` is unsigned).
fn hex_inputs() -> Vec<Vec<u8>> {
    let mut inputs: Vec<Vec<u8>> = [
        &b""[..],
        b"0",
        b"0x",
        b"0X",
        b"0xz",
        b"z",
        b"g",
        b"G",
        b"Zz",
        b"FFtail",
        b"ff!",
        b" 0x10",
        b"+ff",
        b"-1",
        b"1_2",
        b"1.2",
        b"0x0x1",
        b"1\x002",
        b"0x\x00ff",
    ]
    .map(<[u8]>::to_vec)
    .into();
    inputs.push(vec![b'f'; 16]);
    inputs.push(vec![b'f'; 17]);
    inputs.push(vec![b'z'; 256]);
    inputs.push([&b"0x"[..], &[b'F'; 4096]].concat());
    inputs.extend((1..=if UNSIGNED_CHAR { 255 } else { 127 }).map(|byte| vec![byte]));
    inputs
}

/// `cases.py`'s independent value for a hex input in the C locale: an optional `0x`,
/// which counts even with no digits after it, then digits and ASCII letters as 0 to 35,
/// wrapping at the `unsigned long` width.
fn hex_expected(input: &[u8]) -> (c_ulong, usize) {
    let input = input.split(|byte| *byte == 0).next().unwrap_or_default();
    let prefixed = input
        .get(..2)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"0x"));
    let mut index = if prefixed { 2 } else { 0 };
    let mut value: c_ulong = 0;
    while let Some(byte) = input.get(index) {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'A'..=b'Z' => byte - b'A' + 10,
            b'a'..=b'z' => byte - b'a' + 10,
            _ => break,
        };
        value = value.wrapping_mul(16).wrapping_add(c_ulong::from(digit));
        index += 1;
    }
    (value, index)
}

/// Run `check` under each of `cases.py`'s locales this host has, in the C locale
/// afterwards. Returns how many it lacks: the gate counted their cases as unavailable,
/// never as passed.
#[expect(
    clippy::print_stderr,
    reason = "an unavailable locale is reported, as the gate reported it"
)]
fn in_each_locale(mut check: impl FnMut(&CStr)) -> usize {
    let mut unavailable = 0;
    for locale in LOCALES {
        if set_locale(locale) {
            check(locale);
        } else {
            eprintln!("locale {locale:?}: unavailable");
            unavailable += 1;
        }
    }
    assert!(set_locale(c"C"), "the C locale");
    assert!(unavailable <= 3, "the C and POSIX locales are available");
    unavailable
}

// `cases.py`'s corpus, an operation a test, in its order. The pinned bodies stand where
// the gate's complete original `base.cc` binary stood, and the typed Rust functions
// where its hybrid binary called them through `swoole.h`. Each asserts its share of the
// corpus's size.

/// `type`: every 7-bit `char`.
#[test]
fn corpus_type() {
    let _guard = exclusive();
    let mut cases = 0;
    for kind in 0..128_u8 {
        let want = match kind {
            b'c' | b'C' => 1,
            b's' | b'S' | b'n' | b'v' => 2,
            b'l' | b'L' | b'N' | b'V' => 4,
            b'q' | b'Q' | b'J' | b'P' => 8,
            _ => 0,
        };
        let kind = c_char::from_ne_bytes([kind]);
        let rust = observed(0, || (RUST.type_size)(kind));
        assert_eq!(rust, observed(0, || (PINNED.type_size)(kind)), "{kind}");
        assert_eq!(rust.0, want, "{kind}");
        cases += 1;
    }
    assert_eq!(cases, 128);
}

/// `gcd`: the pin subtracts, so pairs that would take over 100,000 steps are left out.
#[test]
fn corpus_gcd() {
    let _guard = exclusive();
    let mut cases = 0;
    for (u, v) in PAIRS
        .into_iter()
        .chain([(2_147_483_648, 1_073_741_824), (196_605, 131_070)])
    {
        let (mut a, mut b, mut steps) = (u64::from(u), u64::from(v), 0);
        while b != 0 {
            steps += a / b;
            (a, b) = (b, a % b);
        }
        if steps > 100_000 {
            continue;
        }
        let rust = observed(0, || (RUST.common_divisor)(u, v));
        let pin = observed(0, || (PINNED.common_divisor)(u, v));
        assert_eq!(rust, pin, "{u} {v}");
        assert_eq!(u64::from(rust.0), gcd(u.into(), v.into()), "{u} {v}");
        cases += 1;
    }
    assert_eq!(cases, 9);
}

/// `lcm`: the pin's remainder is an `int`, so a remainder above `INT_MAX` is left out;
/// the `uint32_t` product wraps before the division.
#[test]
fn corpus_lcm() {
    let _guard = exclusive();
    let mut cases = 0;
    for (u, v) in PAIRS.into_iter().chain([
        (65536, 65536),
        (65536, 131_072),
        (100_000, 100_000),
        (4_294_967_295, 2),
        (2, 4_294_967_295),
        (2_147_483_648, 2_147_483_648),
    ]) {
        let (mut a, mut b, mut valid) = (u, v, true);
        while a % b != 0 {
            let remainder = a % b;
            if remainder > 2_147_483_647 {
                valid = false;
                break;
            }
            (a, b) = (b, remainder);
        }
        if !valid {
            continue;
        }
        let rust = observed(0, || (RUST.common_multiple)(u, v));
        let pin = observed(0, || (PINNED.common_multiple)(u, v));
        assert_eq!(rust, pin, "{u} {v}");
        let product = (u64::from(u) * u64::from(v)) & 0xffff_ffff;
        let want = product / gcd(u.into(), v.into());
        assert_eq!(u64::from(rust.0), want, "{u} {v}");
        cases += 1;
    }
    assert_eq!(cases, 15);
}

/// `radix`: `swoole_dec2hex` in every base, with errno read before the digits are freed.
#[test]
fn corpus_radix() {
    let _guard = exclusive();
    let mut cases = 0;
    for base in 2..=36_u8 {
        let b = c_ulong::from(base);
        let mut values = vec![
            0,
            1,
            b - 1,
            b,
            b + 1,
            b * b - 1,
            b * b,
            c_ulong::MAX,
            c_ulong::MAX - 1,
            1 << 63,
        ];
        values.sort_unstable();
        values.dedup();
        for value in values {
            let mut want = Vec::new();
            let mut rest = value;
            loop {
                let digit = usize::try_from(rest % b).unwrap();
                want.insert(0, b"0123456789abcdefghijklmnopqrstuvwxyz"[digit]);
                rest /= b;
                if rest == 0 {
                    break;
                }
            }
            let base = c_int::from(base);
            let (rust, rust_errno) = observed(0, || (RUST.dec2hex)(value, base));
            let (pin, pin_errno) = observed(0, || (PINNED.dec2hex)(value, base));
            let (rust, pin) = (digits(rust), digits(pin));
            assert_eq!((&rust, rust_errno), (&pin, pin_errno), "{value} {base}");
            assert_eq!(rust, want, "{value} {base}");
            cases += 1;
        }
    }
    assert_eq!(cases, 348);
}

/// `decimal`: `swoole_itoa`, without `LONG_MIN`, whose negation overflows in the pin.
#[test]
fn corpus_decimal() {
    let _guard = exclusive();
    let mut values = vec![
        0,
        1,
        -1,
        c_long::MAX,
        -c_long::MAX,
        c_long::MAX - 1,
        -c_long::MAX + 1,
    ];
    for power in 1..=18 {
        for delta in [-1, 0, 1] {
            let value = c_long::pow(10, power) + delta;
            values.extend([value, -value]);
        }
    }
    values.sort_unstable();
    values.dedup();
    assert_eq!(values.len(), 115);
    for value in values {
        let rust = observed(0, || (RUST.itoa)(value));
        assert_eq!(rust, observed(0, || (PINNED.itoa)(value)), "{value}");
        let want = value.to_string().into_bytes();
        let length = c_int::try_from(want.len()).unwrap();
        assert_eq!(rust.0, (length, want, 0), "{value}");
    }
}

/// `hex`: `swoole_hex2dec` from errno 7 in each locale; the independent value holds in
/// the C and POSIX locales, where only ASCII letters change case.
#[test]
fn corpus_hex() {
    let _guard = exclusive();
    let inputs = hex_inputs();
    assert_eq!(inputs.len(), if UNSIGNED_CHAR { 278 } else { 150 });
    let (mut cases, mut expected) = (0, 0);
    let unavailable = in_each_locale(|locale| {
        for input in &inputs {
            let input = text(input);
            let rust = observed(7, || (RUST.hex2dec)(&input));
            let pin = observed(7, || (PINNED.hex2dec)(&input));
            assert_eq!(rust, pin, "{input:?} {locale:?}");
            cases += 1;
            if [c"C", c"POSIX"].contains(&locale) {
                let want = hex_expected(input.to_bytes());
                assert_eq!(rust.0, want, "{input:?} {locale:?}");
                expected += 1;
            }
        }
    });
    assert_eq!(cases + unavailable * inputs.len(), 5 * inputs.len());
    assert_eq!(expected, 2 * inputs.len());
}

/// `version`: libc's `strtoul` is the parser on both sides, so the result and the errno
/// it leaves are compared in the same locale, from errno 0 and from errno 7.
#[test]
fn corpus_version() {
    let _guard = exclusive();
    let maximum = c_ulong::MAX.to_string();
    let above = (u128::from(c_ulong::MAX) + 1).to_string();
    let nines = "9".repeat(80);
    let versions: Vec<CString> = [
        &b""[..],
        b"0",
        b"1",
        b"01",
        b"1.0",
        b"1.0.0",
        b"1..2",
        b"1x2",
        b"1-2",
        b" 1",
        b"+1",
        b"-1",
        maximum.as_bytes(),
        above.as_bytes(),
        nines.as_bytes(),
        b"x",
        b".",
        b"1\x0099",
        b"1e3",
        b"1\t2",
    ]
    .map(text)
    .into();
    let mut cases = 0;
    let unavailable = in_each_locale(|locale| {
        for left in &versions {
            for right in &versions {
                for initial in [0, 7] {
                    assert_eq!(
                        observed(initial, || (RUST.version_compare)(left, right)),
                        observed(initial, || (PINNED.version_compare)(left, right)),
                        "{left:?} {right:?} {locale:?} from errno {initial}"
                    );
                    cases += 1;
                }
            }
        }
    });
    assert_eq!(cases + unavailable * 800, 4000);
}

/// `envstoi`: the pinned `swoole_get_env` (`std::stoi`) from errno 34 against
/// `parse_stoi`, which is none exactly where `std::stoi` throws, and `get_env`, which
/// reads the variable itself.
#[test]
fn corpus_envstoi() {
    let _guard = exclusive();
    let name = c"SWR_M19_TEST";
    let values: Vec<CString> = [
        "",
        "0",
        "+0",
        "-0",
        "  +42tail",
        "\t-2147483648x",
        "2147483647",
        "2147483648",
        "-2147483649",
        "x",
        "  ",
        "+",
        "--1",
        "12.34",
    ]
    .map(String::from)
    .into_iter()
    .chain([
        c_long::MIN.to_string(),
        c_long::MAX.to_string(),
        (i128::from(c_long::MIN) - 1).to_string(),
        (i128::from(c_long::MAX) + 1).to_string(),
    ])
    .map(|value| text(value.as_bytes()))
    .collect();
    let mut cases = 0;
    let unavailable = in_each_locale(|locale| {
        for value in &values {
            // SAFETY: two NUL-terminated strings; `PROCESS` keeps other tests off the environment.
            let set = unsafe { libc::setenv(name.as_ptr(), value.as_ptr(), 1) };
            assert_eq!(set, 0, "setenv");
            // The driver's `int value = 0x13579bdf`.
            let mut output = 0x1357_9bdf;
            // SAFETY: a NUL-terminated name and a live output.
            let (status, pin_errno) = observed(34, || unsafe {
                pinned::get_env(name.as_ptr(), &raw mut output)
            });
            let (parsed, rust_errno) = observed(34, || parse_stoi(value.to_bytes()));
            if status == 1 {
                let pin = (Some(output), pin_errno);
                assert_eq!((parsed, rust_errno), pin, "{value:?} {locale:?}");
                assert_eq!(observed(34, || get_env(name)), pin, "{value:?} {locale:?}");
            } else {
                // `std::stoi` threw, with the output left alone.
                assert!(matches!(status, -1 | -2), "{value:?} {locale:?}: {status}");
                let untouched = (None, 0x1357_9bdf);
                assert_eq!((parsed, output), untouched, "{value:?} {locale:?}");
            }
            cases += 1;
        }
    });
    // SAFETY: a NUL-terminated name.
    unsafe { libc::unsetenv(name.as_ptr()) };
    assert_eq!(cases + unavailable * 18, 90);
}

/// `allocator.cc`'s `main`, on each engine: one allocation of the exact length, a
/// reentrant helper call inside it, `ENOMEM` and null when it fails, and a retry.
#[test]
fn dec2hex_allocates_once_through_strndup() {
    let _guard = exclusive();
    assert!(set_locale(c"C"));
    REENTER.set(true);
    for engine in [RUST, PINNED] {
        let name = engine.name;
        ALLOCATIONS.set(0);
        let result = (engine.dec2hex)(2_684_326_179, 16);
        assert_eq!(digits(result), b"9fff9123", "{name}");
        assert_eq!((ALLOCATIONS.get(), REQUESTED.get()), (1, 8), "{name}");
        FAIL_ALLOCATION.set(true);
        set_errno(0);
        let result = (engine.dec2hex)(0, 2);
        FAIL_ALLOCATION.set(false);
        assert!(result.is_null(), "{name}");
        assert_eq!(
            (ALLOCATIONS.get(), REQUESTED.get(), errno()),
            (2, 1, libc::ENOMEM),
            "{name}"
        );
        let result = (engine.dec2hex)(0, 36);
        assert_eq!(digits(result), b"0", "{name}");
        assert_eq!((ALLOCATIONS.get(), REQUESTED.get()), (3, 1), "{name}");
    }
    REENTER.set(false);
}

// Upstream's `core-tests/src/core/base.cpp` TESTs the gate extracted and ran on each
// engine, one test each with the same inputs and expected values.

#[test]
fn base_dec2hex() {
    let _guard = exclusive();
    for engine in [RUST, PINNED] {
        let result = (engine.dec2hex)(2_684_326_179, 16);
        assert_eq!(digits(result), b"9fff9123", "{}", engine.name);
    }
}

#[test]
fn base_hex2dec() {
    let _guard = exclusive();
    for engine in [RUST, PINNED] {
        let name = engine.name;
        assert_eq!((engine.hex2dec)(c"9fff9123"), (2_684_326_179, 8), "{name}");
        assert_eq!(
            (engine.hex2dec)(c"0x9fff9123"),
            (2_684_326_179, 10),
            "{name}"
        );
        assert_eq!((engine.hex2dec)(c"f"), (15, 1), "{name}");
    }
}

#[test]
fn base_version_compare() {
    let _guard = exclusive();
    for engine in [RUST, PINNED] {
        let name = engine.name;
        assert_eq!((engine.version_compare)(c"1.2.1", c"1.2.0"), 1, "{name}");
        assert_eq!((engine.version_compare)(c"1.2.3", c"1.3.0"), -1, "{name}");
        assert_eq!((engine.version_compare)(c"1.2.3", c"1.2.9"), -1, "{name}");
        assert_eq!((engine.version_compare)(c"1.2.0", c"1.2.0"), 0, "{name}");
    }
}

#[test]
fn base_common_divisor() {
    for engine in [RUST, PINNED] {
        let name = engine.name;
        assert_eq!((engine.common_divisor)(16, 12), 4, "{name}");
        assert_eq!((engine.common_divisor)(6, 15), 3, "{name}");
        assert_eq!((engine.common_divisor)(32, 16), 16, "{name}");
    }
}

#[test]
fn base_common_multiple() {
    for engine in [RUST, PINNED] {
        let name = engine.name;
        assert_eq!((engine.common_multiple)(16, 12), 48, "{name}");
        assert_eq!((engine.common_multiple)(6, 15), 30, "{name}");
        assert_eq!((engine.common_multiple)(32, 16), 32, "{name}");
    }
}

#[test]
fn base_itoa() {
    for engine in [RUST, PINNED] {
        let (n, text, terminator) = (engine.itoa)(123_456_987);
        assert_eq!(n, 9, "{}", engine.name);
        assert_eq!((text.as_slice(), terminator), (&b"123456987"[..], 0));
    }
}

#[test]
fn base_type_size() {
    for engine in [RUST, PINNED] {
        let size = |kind: u8| (engine.type_size)(c_char::from_ne_bytes([kind]));
        let name = engine.name;
        assert_eq!(size(b'c'), 1, "{name}");
        assert_eq!(size(b's'), 2, "{name}");
        assert_eq!(size(b'l'), 4, "{name}");
        assert_eq!(size(b'b'), 0, "{name}");
        assert_eq!(size(b'q'), 8, "{name}");
        assert_eq!(size(b'P'), 8, "{name}");
    }
}
