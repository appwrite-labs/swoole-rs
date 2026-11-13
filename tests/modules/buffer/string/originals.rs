// SPDX-License-Identifier: Apache-2.0
//! Upstream's `core-tests/src/core/string.cpp`, one Rust test per `TEST` over
//! `swoole::String` (README.md maps them), and the project's `compatibility.cc`
//! checks. Each C++ call is the Rust method that is its body.
use super::tests::new;
use super::*;
use std::ffi::CStr;

const TEST_STR: &[u8] = b"hello world, hello swoole\n";
const TEST_STR2: &[u8] = b"I am Rango\n";
const INIT_SIZE: usize = 1024;
const TEST_DATA: &[u8] = b"hello,world,swoole,php,last";

/// The gates' `SwooleG.pagesize = 4096`, which a growing `write` rounds to.
fn page_size() {
    swoole_rs_corebase::swr_v1_core_set_pagesize(4096);
}

/// `ASSERT_MEMEQ(s.str, expected, s.length)`.
fn assert_memeq(s: &CppString, expected: &[u8]) {
    assert_eq!(s.view(), &expected[..s.length]);
}

/// `String copy(source)`.
fn copy_of(source: &CppString) -> CppString {
    let mut s = MaybeUninit::uninit();
    swr_string_copy(&mut s, source);
    unsafe { s.assume_init() }
}

/// `this = source`, between two Strings.
fn assign_copy(this: &mut CppString, source: &CppString) {
    this.dispose();
    this.copy_from(source);
}

/// A String over `text`, which it does not own: upstream sets `str` and `length`.
fn borrowed(text: &'static CStr) -> CppString {
    CppString {
        length: text.to_bytes().len(),
        size: 0,
        offset: 0,
        str: text.as_ptr().cast_mut().cast(),
        allocator: None,
    }
}

fn chunk<'a>(data: *const u8, length: usize) -> &'a [u8] {
    unsafe { std::slice::from_raw_parts(data, length) }
}

#[test]
fn move_and_copy() {
    let mut s1 = new(TEST_STR);
    assert_memeq(&s1, TEST_STR);

    let mut s2 = copy_of(&s1);
    assert_memeq(&s2, TEST_STR);
    assert!(!s1.str.is_null());

    let mut s3 = MaybeUninit::uninit();
    swr_string_move(&mut s3, &mut s1);
    let mut s3 = unsafe { s3.assume_init() };
    assert_memeq(&s3, TEST_STR);
    assert!(s1.str.is_null());

    let mut s4 = CppString::EMPTY;
    assign_copy(&mut s4, &s3);
    assert_memeq(&s4, TEST_STR);
    assert!(!s3.str.is_null());

    let mut s5 = CppString::EMPTY;
    s5.dispose();
    s5.move_from(&mut s3);
    assert_memeq(&s5, TEST_STR);
    assert!(s3.str.is_null());

    let mut s6 = new(TEST_STR);
    assert_memeq(&s6, TEST_STR);

    for s in [&mut s1, &mut s2, &mut s3, &mut s4, &mut s5, &mut s6] {
        s.dispose();
    }
}

#[test]
fn append() {
    let mut s1 = new(TEST_STR);
    swr_string_append_int(&mut s1, 12_345_678);

    let mut s2 = new(TEST_STR2);
    s1.append(s2.view());

    assert_memeq(&s1, &[TEST_STR, b"12345678", TEST_STR2].concat());
    s1.dispose();
    s2.dispose();
}

#[test]
fn write() {
    page_size();
    let mut s1 = CppString::EMPTY;
    s1.reserve(32);

    let mut s2 = new(TEST_STR);
    s1.repeat(b" ", 30);
    swr_string_write_string(&mut s1, 30, &s2);

    let mut s3 = swr_string_substr(&s1, 30, s2.length);
    assert_memeq(&s3, TEST_STR);
    for s in [&mut s1, &mut s2, &mut s3] {
        s.dispose();
    }
}

#[test]
fn repeat() {
    let end_str = b"[end]";
    let mut s1 = CppString::EMPTY;
    s1.repeat(b"hello\r\n", 5);
    s1.append(end_str);

    let mut count = 0;
    let offset = s1.split(b"\r\n", |data, _| {
        count += 1;
        assert_eq!(chunk(data, 7), b"hello\r\n");
        true
    });

    assert_eq!(offset.cast_unsigned(), s1.length - end_str.len());
    assert_eq!(
        &s1.view()[offset.cast_unsigned()..][..end_str.len()],
        end_str
    );

    assert_eq!(count, 5);
    s1.dispose();
}

#[test]
fn release() {
    let mut s1 = new(TEST_STR);
    assert_memeq(&s1, TEST_STR);

    let s2 = s1.release();
    assert!(s1.str.is_null());
    assert_eq!(s1.length, 0);

    assert_eq!(chunk(s2, TEST_STR.len()), TEST_STR);
    unsafe { libc::free(s2.cast()) };
}

/// Upstream also runs `s1 = s1`, a no-op its operator checks for; two borrows of one
/// String cannot be written here.
#[test]
fn ub() {
    let mut s1 = new(TEST_STR);
    let mut s2 = new(TEST_STR2);

    assign_copy(&mut s1, &s2);

    let rs = swr_string_substr(&s1, s1.length, 10);
    assert!(rs.str.is_null());
    s1.dispose();
    s2.dispose();
}

#[test]
fn explode() {
    let haystack = c"hello world";
    let mut str = borrowed(haystack);

    let mut value_1 = 0;
    let mut explode_str: *const u8 = ptr::null();
    let mut explode_length = 0;

    str.split(b" ", |data, length| {
        explode_str = data;
        explode_length = length;
        value_1 = 5;
        false
    });

    assert_eq!(haystack, unsafe { CStr::from_ptr(explode_str.cast()) });
    assert_eq!(6, explode_length);
    assert_eq!(5, value_1);
}

#[test]
fn explode_2() {
    let mut str = borrowed(c"hello,world,swoole,php,last");

    let mut count = 0;
    let mut list: Vec<Vec<u8>> = Vec::new();

    let n = str
        .split(b",", |data, length| {
            list.push(chunk(data, length - 1).to_vec());
            count += 1;
            true
        })
        .cast_unsigned();

    assert_eq!(list[0], b"hello");
    assert_eq!(list[1], b"world");
    assert_eq!(list[2], b"swoole");
    assert_eq!(list[3], b"php");
    assert_eq!(b"last", &str.view()[n..]);
    assert_eq!(4, count);
    assert_eq!(list.len(), count);
}

#[test]
fn pop_1() {
    let s = new_sized_string(INIT_SIZE, None);
    let str = unsafe { &mut *s };

    let str_1 = str.str;

    let len_1 = 11;
    str.append(TEST_DATA);
    str.offset = 11;
    let str_2 = str.pop(INIT_SIZE);

    assert_eq!(str_1, str_2);
    assert_eq!(b"hello,world", chunk(str_2, len_1));
    assert_eq!(b",swoole,php,last", str.view());
    assert_eq!(INIT_SIZE, str.size);

    unsafe { (str.allocator.unwrap().free)(str_1.cast()) };
    unsafe { delete_string(s) };
}

#[test]
fn pop_2() {
    let s = new_sized_string(INIT_SIZE, None);
    let str = unsafe { &mut *s };

    let str_1 = str.str;

    let len_1 = TEST_DATA.len();
    str.append(TEST_DATA);
    str.offset = signed(len_1);
    let str_2 = str.pop(INIT_SIZE);

    assert_eq!(str_1, str_2);
    assert_eq!(TEST_DATA, chunk(str_2, len_1));
    assert_eq!(str.length, 0);
    assert_eq!(INIT_SIZE, str.size);

    unsafe { (str.allocator.unwrap().free)(str_1.cast()) };
    unsafe { delete_string(s) };
}

#[test]
fn reduce_1() {
    let s = new_sized_string(INIT_SIZE, None);
    let str = unsafe { &mut *s };

    str.append(TEST_DATA);
    str.offset = 11;

    str.reduce(str.offset);

    assert_eq!(b",swoole,php,last", str.view());
    unsafe { delete_string(s) };
}

#[test]
fn reduce_2() {
    let s = new_sized_string(INIT_SIZE, None);
    let str = unsafe { &mut *s };

    str.append(TEST_DATA);
    str.offset = signed(str.length);

    str.reduce(str.offset);

    assert_eq!(str.length, 0);
    unsafe { delete_string(s) };
}

#[test]
fn reduce_3() {
    let s = new_sized_string(INIT_SIZE, None);
    let str = unsafe { &mut *s };

    str.append(TEST_DATA);
    str.offset = 0;

    str.reduce(str.offset);

    assert_eq!(str.length, TEST_DATA.len());
    unsafe { delete_string(s) };
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no printf, which `print` calls")]
fn append_number() {
    let data = b"hello";
    let s = new_sized_string(data.len() + 32, None);
    let str = unsafe { &mut *s };
    str.append(data);
    swr_string_append_int(str, 123);
    str.set_null_terminated();
    assert_eq!(unsafe { CStr::from_ptr(str.str.cast()) }, c"hello123");

    str.print(true);
    str.print(false);

    unsafe { delete_string(s) };
}

/// `compatibility.cc`: ordinary default-allocator operations, with its count of checks.
#[test]
fn compatibility() {
    page_size();
    let mut checks = 0;
    let mut check = |value: bool| {
        assert!(value, "Positive compatibility assertion");
        checks += 1;
    };
    let sized = |size| {
        let mut s = MaybeUninit::uninit();
        swr_string_new(&mut s, size, None);
        unsafe { s.assume_init() }
    };

    let mut append = sized(8);
    append.append(b"abc");
    append.offset = 1;
    append.append(b"defghijkl");
    check(append.length == 12);
    check(append.size == 16);
    check(append.offset == 1);
    check(&append.view()[..12] == b"abcdefghijkl");
    let mut source = new(b"XYZ");
    append.append(source.view());
    check(append.length == 15);
    check(&append.view()[12..15] == b"XYZ");

    let mut write = new(b"abc");
    write.offset = 1;
    write.write(1, b"XYZ");
    check(write.length == 4);
    check(write.offset == 1);
    check(&write.view()[..4] == b"aXYZ");

    let mut integer = sized(8);
    integer.append(b"a");
    swr_string_append_int(&mut integer, 1_234_567_890);
    check(integer.length == 11);
    check(&integer.view()[..11] == b"a1234567890");

    let mut repeated = sized(8);
    repeated.repeat(b"abcdefghijklmnopq", 3);
    check(repeated.length == 51);
    check(repeated.size == 56);
    for i in 0..3 {
        check(&repeated.view()[i * 17..][..17] == b"abcdefghijklmnopq");
    }

    let mut single = sized(8);
    single.repeat(b"Z", 17);
    check(single.length == 17);
    check(single.size == 24);
    for i in 0..17 {
        check(single.view()[i] == b'Z');
    }

    source.offset = 1;
    let mut copied = copy_of(&source);
    check(copied.length == 3);
    check(copied.offset == 1);
    check(&copied.view()[..3] == b"XYZ");

    let mut terminated = sized(8);
    terminated.append(b"abcdefgh");
    terminated.set_null_terminated();
    check(terminated.length == 8);
    check(unsafe { *terminated.str.add(8) } == 0);
    check(terminated.size == 16);

    let mut reduced = new(b"abcdef");
    reduced.offset = 1;
    reduced.reduce(2);
    check(reduced.length == 4);
    check(reduced.offset == 0);
    check(&reduced.view()[..4] == b"cdef");

    assert_eq!(checks, 44);
    for s in [
        &mut append,
        &mut source,
        &mut write,
        &mut integer,
        &mut repeated,
        &mut single,
        &mut copied,
        &mut terminated,
        &mut reduced,
    ] {
        s.dispose();
    }
}
