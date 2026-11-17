//! The request oracle is called only after the independent bounds model accepts.
use super::{resp_in_bounds, resp_server};

#[test]
fn bounded_request_forms_match_the_pin() {
    for wire in [
        b"*0\r\n".as_slice(),
        b"*3\r\n:7\r\n$3\r\nSET\r\n$-1\r\n",
        b"*1\r\n:9223372036854775808\r\n",
        b":-9223372036854775809\r\n",
        b"xy",
        b"$1\r\na\r\nb\r\n",
        b"$\r\nxy",
        b"$4294967296\r\n\r\n",
        b"$3\r\na\0b\r\n",
        b": \t-7\r\n",
    ] {
        assert!(resp_in_bounds(wire, true), "{wire:?}");
        resp_server(wire);
    }
    assert!(
        !resp_in_bounds(b"xy", false),
        "reply parser has no prefix fallthrough"
    );
}

#[test]
fn incomplete_requests_keep_the_oracle_outside_the_domain() {
    for wire in [
        b"".as_slice(),
        b"*",
        b":oops\r\n",
        b"$-2\r\nx\r\n",
        b"$3\r\nab",
        b"$2\r\nx\r\n",
        b"\0",
    ] {
        assert!(!resp_in_bounds(wire, true), "{wire:?}");
        resp_server(wire);
    }
}
