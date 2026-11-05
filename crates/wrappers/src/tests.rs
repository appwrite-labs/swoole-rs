//! The policies the wrappers gates checked from C (`tests/modules/wrappers`, #502),
//! against upstream v6.2.4's `src/wrapper/{event,http}.cc` expressions.
use super::*;
use std::ffi::{c_char, c_int};

unsafe extern "C" {
    fn snprintf(buf: *mut c_char, size: usize, format: *const c_char, ...) -> c_int;
}

/// A xorshift stream, so failures reproduce.
fn stream(mut seed: u64) -> impl FnMut() -> u64 {
    move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    }
}

fn random(next: &mut impl FnMut() -> u64, max: u64) -> Vec<u8> {
    (0..next() % max).map(|_| next().to_le_bytes()[0]).collect()
}

fn view(p: *const Request) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, u32) {
    let mut v = View::default();
    assert_eq!(unsafe { swr_v1_wrappers_request_view(p, &raw mut v) }, 0);
    let span = |s: Span| unsafe { slice::from_raw_parts(s.data, s.len) }.to_vec();
    (
        span(v.header),
        span(v.form),
        span(v.input),
        span(v.body),
        v.flags,
    )
}

#[test]
fn event_direction_wait_and_init_match_the_native_expressions() {
    // SW_EVENT_READ is 512 and SW_EVENT_WRITE 1024.
    for event in -2048..=2048 {
        let expected = match event {
            512 => 1,
            1024 => 2,
            _ => 0,
        };
        assert_eq!(swr_v1_wrappers_event_direction(event), expected, "{event}");
    }
    // `!(wait_exit && if_exit())`, with if_exit evaluated only under wait_exit.
    for wait in 0..2 {
        for exit in 0..2 {
            assert_eq!(
                swr_v1_wrappers_event_wait(wait, exit),
                u8::from(wait == 0 || exit == 0)
            );
        }
    }
    let refused = swr_v1_wrappers_event_init(u32::MAX, u32::MAX, 0);
    assert_eq!(
        (refused.accepted, refused.wait_exit, refused.initializers),
        (0, 0, [0; 3])
    );
    for flags in [0, 1, 4, 6] {
        let plan = swr_v1_wrappers_event_init(flags, 4, 1);
        assert_eq!(plan.accepted, 1);
        assert_eq!(plan.wait_exit, u32::from(flags & 4 != 0), "{flags}");
        assert_eq!(plan.initializers, [1, 2, 3]);
    }
}

#[test]
fn header_lines_match_sw_snprintf_and_split_matches_find() {
    let mut next = stream(0x81f8_3e95);
    for _ in 0..20_000 {
        let (key, value) = (random(&mut next, 1600), random(&mut next, 1600));
        let cap = (next() % 1100) as usize;
        let mut expected = vec![42_u8; cap + 1];
        let mut actual = expected.clone();
        let wanted = {
            let (mut k, mut v) = (key.clone(), value.clone());
            k.push(0);
            v.push(0);
            let n = unsafe {
                snprintf(
                    expected.as_mut_ptr().cast(),
                    cap,
                    c"%s: %s\r\n".as_ptr(),
                    k.as_ptr(),
                    v.as_ptr(),
                )
            };
            let n = usize::try_from(n).expect("snprintf succeeds");
            if cap == 0 { n } else { n.min(cap - 1) }
        };
        let got = unsafe {
            swr_v1_wrappers_header_wire(
                key.as_ptr(),
                key.len(),
                value.as_ptr(),
                value.len(),
                actual.as_mut_ptr(),
                cap,
            )
        };
        assert_eq!(got, wanted);
        assert_eq!(actual, expected);
        let found = key.iter().position(|&c| c == b'?').unwrap_or(key.len());
        assert_eq!(
            unsafe { swr_v1_wrappers_split(key.as_ptr(), key.len(), b'?') },
            found
        );
    }
}

#[test]
fn listener_plan_matches_the_address_find() {
    let mut next = stream(0x0061_6464_7265_7373);
    let mut inputs: Vec<Vec<u8>> = [
        "",
        ":0",
        "127.0.0.1:9501",
        "localhost",
        "host:",
        "::1:9501",
        ":-1",
        "\0:42",
        "name\0:5",
    ]
    .iter()
    .map(|s| s.as_bytes().to_vec())
    .collect();
    inputs.extend((0..4096).map(|_| random(&mut next, 128)));
    for input in inputs {
        let plan = unsafe { swr_v1_wrappers_listener_plan(input.as_ptr(), input.len()) };
        // `addr.find(':')`, npos rejected, an empty host defaults.
        let found = input.iter().position(|&c| c == b':');
        assert_eq!(plan.separator, found.unwrap_or(input.len()), "{input:?}");
        assert_eq!(plan.valid, u32::from(found.is_some()));
        assert_eq!(plan.default_host, u32::from(found == Some(0)));
    }
    let empty = unsafe { swr_v1_wrappers_listener_plan(ptr::null(), 0) };
    assert_eq!(
        (empty.separator, empty.valid, empty.default_host),
        (0, 0, 0)
    );
}

#[test]
fn response_effects_match_context_end() {
    #[derive(Debug, PartialEq)]
    enum Effect {
        Header,
        Body,
        Keepalive,
        Close,
    }
    // Upstream's Context::end: header send, the body when nonempty, then close unless keepalive.
    fn upstream(length: usize, header: bool, body: bool, keepalive: bool) -> (Vec<Effect>, bool) {
        let mut effects = vec![Effect::Header];
        if !header {
            return (effects, false);
        }
        if length > 0 {
            effects.push(Effect::Body);
            if !body {
                return (effects, false);
            }
        }
        effects.push(Effect::Keepalive);
        if !keepalive {
            effects.push(Effect::Close);
        }
        (effects, true)
    }
    // The patched Context::end's loop over swr_v1_wrappers_response_next.
    fn selected(length: usize, header: bool, body: bool, keepalive: bool) -> (Vec<Effect>, bool) {
        let (mut effects, mut action, mut succeeded, mut observed) = (vec![], 0, 1, 0);
        loop {
            action = swr_v1_wrappers_response_next(action, length, succeeded, observed);
            match action {
                1 => {
                    effects.push(Effect::Header);
                    succeeded = u32::from(header);
                }
                2 => {
                    effects.push(Effect::Body);
                    succeeded = u32::from(body);
                }
                3 => {
                    effects.push(Effect::Keepalive);
                    observed = u32::from(keepalive);
                }
                4 => effects.push(Effect::Close),
                5 => return (effects, true),
                _ => return (effects, false),
            }
        }
    }
    for length in [0, 1, 1024, usize::MAX] {
        for case in 0..8 {
            let (header, body, keepalive) = (case & 1 != 0, case & 2 != 0, case & 4 != 0);
            assert_eq!(
                selected(length, header, body, keepalive),
                upstream(length, header, body, keepalive),
                "{length} {header} {body} {keepalive}"
            );
        }
    }
    assert_eq!(
        swr_v1_wrappers_response_next(99, 0, 1, 1),
        6,
        "an unknown action fails"
    );
}

#[test]
fn content_length_is_std_to_string() {
    let mut next = stream(0x1234_5678);
    let lengths = (0..16_384)
        .map(|i| {
            if i < 1000 {
                i
            } else {
                usize::try_from(next() >> 32).expect("32 bits")
            }
        })
        .chain([usize::MAX]);
    for length in lengths {
        let mut out = [0_u8; usize::BITS as usize];
        let n = unsafe { swr_v1_wrappers_content_length(length, out.as_mut_ptr(), out.len()) };
        assert_eq!(&out[..n], length.to_string().as_bytes());
        let short = unsafe { swr_v1_wrappers_content_length(length, out.as_mut_ptr(), n - 1) };
        assert_eq!(short, usize::MAX, "a short buffer is refused");
    }
    assert_eq!(
        unsafe { swr_v1_wrappers_content_length(1, ptr::null_mut(), 8) },
        usize::MAX
    );
}

#[test]
fn a_request_classifies_headers_and_accumulates_parts() {
    let p = swr_v1_wrappers_request_new();
    assert_eq!(view(p).4, 0);
    let set = |name: &[u8]| {
        assert_eq!(
            unsafe { swr_v1_wrappers_header(p, name.as_ptr(), name.len()) },
            0
        );
    };
    let kind = |method, value: &[u8], multipart| unsafe {
        swr_v1_wrappers_header_kind(p, method, value.as_ptr(), value.len(), multipart)
    };
    set(b"Content-Type");
    // Only DELETE (0), POST (3), PUT (4) and PATCH (28) take a body type.
    for method in 0..50 {
        let allowed = i32::from(matches!(method, 0 | 3 | 4 | 28));
        assert_eq!(
            kind(
                method,
                b"Application/X-WWW-Form-Urlencoded; charset=utf8",
                0
            ),
            allowed
        );
        assert_eq!(
            kind(method, b"MULTIPART/FORM-DATA; boundary=abc", 0),
            allowed * 2
        );
        assert_eq!(kind(method, b"multipart/form-dat", 0), 0);
    }
    set(b"content-disposition");
    assert_eq!(kind(0, b"anything", 1), 3);
    let disposition =
        |value: &[u8]| unsafe { swr_v1_wrappers_disposition(p, value.as_ptr(), value.len()) };
    // Case-sensitive keys, the last duplicate wins.
    assert_eq!(
        disposition(b"form-data; name=first; Name=ignored; name=last"),
        0
    );
    assert_eq!(view(p).1, b"last");
    let data = b"a\0b";
    assert_eq!(
        unsafe { swr_v1_wrappers_body_append(p, data.as_ptr(), data.len()) },
        0
    );
    assert_eq!(view(p).3, data);
    unsafe { swr_v1_wrappers_part_end(p) };
    let (header, form, _, body, _) = view(p);
    assert!(
        header.is_empty() && form.is_empty() && body.is_empty(),
        "a form part clears its body"
    );
    // A filename selects the input name and leaves the form name unset.
    assert_eq!(disposition(b"form-data; name=field; filename=image.jpg"), 0);
    assert_eq!((view(p).1, view(p).2), (vec![], b"image.jpg".to_vec()));
    unsafe { swr_v1_wrappers_part_end(p) };
    // Neither is rejected where upstream dereferences the map's end().
    assert_eq!(disposition(b"form-data"), -1);
    set(b"Swoole-Upload-File");
    assert_eq!(kind(0, b"/tmp/file", 1), 4);
    // The first nonempty body skips its leading CR/LF, once.
    let leading = b"\r\n\r--abc";
    assert_eq!(
        unsafe { swr_v1_wrappers_body_skip(p, leading.as_ptr(), leading.len()) },
        3
    );
    assert_eq!(
        unsafe { swr_v1_wrappers_body_skip(p, leading.as_ptr(), leading.len()) },
        0
    );
    unsafe { swr_v1_wrappers_complete(p) };
    assert_eq!(view(p).4, 1);
    unsafe { swr_v1_wrappers_request_free(p) };
    // Null handles are refused.
    assert_eq!(
        unsafe { swr_v1_wrappers_header(ptr::null_mut(), ptr::null(), 0) },
        -1
    );
    assert_eq!(
        unsafe { swr_v1_wrappers_request_view(ptr::null(), ptr::null_mut()) },
        -1
    );
    unsafe { swr_v1_wrappers_request_free(ptr::null_mut()) };
}
