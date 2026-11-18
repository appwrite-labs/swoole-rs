use super::*;

struct Hooks {
    parser: Parser,
    answer: Flow,
    calls: usize,
}

impl Callbacks<'_> for Hooks {
    fn parser(&mut self) -> &mut Parser {
        &mut self.parser
    }
    fn on_url(&mut self, _: &[u8]) -> Flow {
        self.calls += 1;
        self.answer
    }
    fn on_message_complete(&mut self) -> Flow {
        self.calls += 1;
        self.answer
    }
}

#[test]
fn flushed_spans_retain_arbitrary_callback_codes_and_error_position() {
    let input = b"GET /partial";
    for code in [0, 1, 2, -1, -7, 21, 22, 23, 99, i32::MIN, i32::MAX] {
        let mut hooks = Hooks {
            parser: Parser::new(Kind::Request),
            answer: Flow::from_code(code),
            calls: 0,
        };
        let expected = match code {
            0 => Ok(()),
            -1 => Err(HPE_USER),
            code => Err(code),
        };
        assert_eq!(execute(&mut hooks, input), expected);
        assert_eq!(hooks.calls, 1);
        if let Err(error) = expected {
            assert_eq!(hooks.parser.error, error);
            assert_eq!(hooks.parser.error_pos(), input.len());
            assert_eq!(execute(&mut hooks, b"ignored"), expected);
            assert_eq!(finish(&mut hooks), Ok(()));
            assert_eq!(hooks.calls, 1, "an existing error invokes no callback");
        }
    }
}

#[test]
fn end_of_input_delivers_only_complete_messages() {
    use EndOfInput::{Complete, CompleteAtEof, Truncated};
    for (wire, expected) in [
        (&b""[..], Truncated),
        (b"HTTP/1.1 200", Truncated),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nx", Truncated),
        (
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
            Truncated,
        ),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nx", Complete),
        (b"HTTP/1.1 204 No Content\r\n\r\n", Complete),
        (b"HTTP/1.1 200 OK\r\n\r\nx", CompleteAtEof),
    ] {
        for answer in [Flow::Go, Flow::Pause] {
            let mut hooks = Hooks {
                parser: Parser::new(Kind::Response),
                answer,
                calls: 0,
            };
            for byte in wire {
                let result = execute(&mut hooks, std::slice::from_ref(byte));
                assert!(matches!(result, Ok(()) | Err(HPE_PAUSED)));
            }
            assert_eq!(hooks.end_of_input(), expected);
            assert_eq!(
                hooks.end_of_input(),
                if expected == Truncated {
                    Truncated
                } else {
                    Complete
                }
            );
            assert_eq!(hooks.calls, usize::from(expected != Truncated));
            hooks.parser.reset();
            assert_eq!(hooks.end_of_input(), Truncated);
        }
    }
}

#[test]
fn finish_retains_completion_codes_without_storing_a_parser_error() {
    for code in [0, 1, 2, -1, -7, 21, 22, 23, 99, i32::MIN, i32::MAX] {
        let mut hooks = Hooks {
            parser: Parser::new(Kind::Response),
            answer: Flow::from_code(code),
            calls: 0,
        };
        assert_eq!(execute(&mut hooks, b"HTTP/1.1 200 OK\r\n\r\nbody"), Ok(()));
        assert_eq!(hooks.parser.finish, Finish::SafeWithCb);
        assert_eq!(hooks.calls, 0);
        let expected = if code == 0 { Ok(()) } else { Err(code) };
        assert_eq!(finish(&mut hooks), expected);
        assert_eq!(hooks.calls, 1);
        assert_eq!(hooks.parser.error, 0);
        assert_eq!(hooks.parser.reason(), "");
    }
}

#[test]
fn incomplete_eof_sets_reason_but_does_not_store_an_error() {
    let mut hooks = Hooks {
        parser: Parser::new(Kind::Request),
        answer: Flow::Go,
        calls: 0,
    };
    assert_eq!(execute(&mut hooks, b"GET / HTTP/1.1\r\nHost:"), Ok(()));
    assert_eq!(finish(&mut hooks), Err(HPE_INVALID_EOF_STATE));
    assert_eq!(hooks.parser.reason(), "Invalid EOF state");
    assert_eq!(hooks.parser.error, 0);
    assert_eq!(hooks.calls, 1);
}

#[test]
fn a_previous_response_or_rejected_callback_cannot_complete_the_message() {
    let mut hooks = Hooks {
        parser: Parser::new(Kind::Response),
        answer: Flow::Go,
        calls: 0,
    };
    execute(&mut hooks, b"HTTP/1.1 204 Done\r\n\r\nHTTP/1.1 200").unwrap();
    assert_eq!(hooks.end_of_input(), EndOfInput::Truncated);
    assert_eq!(hooks.calls, 1);

    hooks.parser.reset();
    hooks.answer = Flow::Fail;
    execute(&mut hooks, b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
    assert_eq!(hooks.end_of_input(), EndOfInput::Truncated);
    assert_eq!(hooks.end_of_input(), EndOfInput::Truncated);
    assert_eq!(hooks.calls, 2);
}
