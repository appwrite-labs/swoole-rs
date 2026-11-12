//! Coroutine HTTP server handler routing and accept loop. Native code owns all effects/data.
use std::ffi::CString;
use swoole_rs_locale::case_prefix;

/// Whether a coroutine HTTP server handler's `pattern` routes `path`: a
/// case-insensitive prefix. Native container order and default callback
/// identity remain the caller's.
#[must_use]
pub fn handler_matches(path: &[u8], pattern: &[u8]) -> bool {
    case_prefix(path, pattern)
}

/// Native errno values are supplied by the actual platform, never guessed here.
pub struct AcceptErrors {
    pub per_process: i32,
    pub system: i32,
    pub timeout: i32,
    pub bad_ssl: i32,
    pub canceled: i32,
}

/// What upstream's `start` accept loop does to its server and listening socket.
pub trait AcceptHost {
    /// `hs->running`.
    fn running(&mut self) -> bool;
    /// `sock->accept()`: `None` when it failed, else the `onAccept` coroutine's
    /// cid for the connection.
    fn accept(&mut self) -> Option<i64>;
    /// The socket's `errCode` and `errMsg`.
    fn error(&mut self) -> (i32, CString);
    /// `System::sleep(SW_ACCEPT_RETRY_TIME)`.
    fn sleep(&mut self);
    /// `http_server_coro_set_error`.
    fn set_error(&mut self);
    /// The `E_WARNING` for a failed accept; the host then stops the server.
    fn fail(&mut self, warning: &str);
}

/// Upstream's `start` loop: accept until the server stops. True is
/// `RETURN_TRUE`, false `RETURN_FALSE` after a failed accept.
pub fn accept_loop(host: &mut impl AcceptHost, errors: &AcceptErrors) -> bool {
    while host.running() {
        if let Some(cid) = host.accept() {
            if cid < 0 {
                host.sleep();
            }
            continue;
        }
        let (error, message) = host.error();
        if error == errors.per_process || error == errors.system {
            host.sleep();
        } else if error == errors.timeout || error == errors.bad_ssl {
        } else if error == errors.canceled {
            host.set_error();
            break;
        } else {
            let warning = format!(
                "accept failed, Error: {}[{error}]",
                message.to_string_lossy()
            );
            host.set_error();
            host.fail(&warning);
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    static ERRORS: AcceptErrors = AcceptErrors {
        per_process: 24,
        system: 23,
        timeout: 110,
        bad_ssl: 1012,
        canceled: 125,
    };

    /// One accept outcome (connected, cid, errCode) per round, until none are
    /// left and the server stops; the effects in order.
    struct Script {
        rounds: Vec<(bool, i64, i32)>,
        trace: Vec<String>,
    }

    impl AcceptHost for Script {
        fn running(&mut self) -> bool {
            !self.rounds.is_empty()
        }
        fn accept(&mut self) -> Option<i64> {
            self.trace.push("accept".into());
            let (connection, cid, _) = self.rounds[0];
            connection.then(|| {
                self.rounds.remove(0);
                cid
            })
        }
        fn error(&mut self) -> (i32, CString) {
            (self.rounds.remove(0).2, c"boom".to_owned())
        }
        fn sleep(&mut self) {
            self.trace.push("sleep".into());
        }
        fn set_error(&mut self) {
            self.trace.push("set-error".into());
        }
        fn fail(&mut self, warning: &str) {
            self.trace.push(warning.into());
        }
    }

    fn run(rounds: &[(bool, i64, i32)]) -> (bool, Vec<String>) {
        let mut script = Script {
            rounds: rounds.to_vec(),
            trace: Vec::new(),
        };
        let result = accept_loop(&mut script, &ERRORS);
        (result, script.trace)
    }

    fn trace(events: &[&str]) -> Vec<String> {
        events.iter().map(|event| (*event).to_owned()).collect()
    }

    #[test]
    fn accept_loop_follows_upstreams_branches() {
        assert_eq!(
            run(&[]),
            (true, trace(&[])),
            "a stopped server accepts nothing"
        );
        let retries = [
            (true, 5, 0),
            (true, -1, 0),
            (false, 0, 24),
            (false, 0, 23),
            (false, 0, 110),
            (false, 0, 1012),
        ];
        let expected = [
            "accept", "accept", "sleep", "accept", "sleep", "accept", "sleep",
        ];
        assert_eq!(
            run(&retries),
            (
                true,
                trace(&[&expected[..], &["accept", "accept"]].concat())
            ),
            "a failed coroutine and EMFILE/ENFILE sleep; timeouts and bad TLS clients retry"
        );
        assert_eq!(
            run(&[(false, 0, 125), (true, 1, 0)]),
            (true, trace(&["accept", "set-error"])),
            "a cancel records the error and stops accepting"
        );
        assert_eq!(
            run(&[(false, 0, 9), (true, 1, 0)]),
            (
                false,
                trace(&["accept", "set-error", "accept failed, Error: boom[9]"])
            ),
            "any other error records it, warns and returns false"
        );
    }
}
