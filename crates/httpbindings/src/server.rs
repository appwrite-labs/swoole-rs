//! Coroutine HTTP server handler routing and accept loop. Native code owns all effects/data.
use crate::Text;
use crate::request::{bytes, case_prefix};
use core::ffi::{CStr, c_char, c_void};

/// # Safety
/// Spans contain initialized readable bytes for the call. Native container order
/// and default callback identity remain authoritative; default entries are skipped
/// by the native custody guard before observing a potentially absent context path.
#[must_use]
pub unsafe fn swr_v1_httpbindings_handler(path: Text, pattern: Text) -> i32 {
    // SAFETY: `path` spans `length` initialized bytes (null only when empty) for this call.
    let path = unsafe { bytes(path) };
    // SAFETY: `pattern` spans `length` initialized bytes (null only when empty) for this call.
    let pattern = unsafe { bytes(pattern) };
    let (Some(path), Some(pattern)) = (path, pattern) else {
        return -1;
    };
    i32::from(case_prefix(path, pattern))
}
/// Native errno values are supplied by the actual platform, never guessed here.
#[repr(C)]
pub struct AcceptErrors {
    pub per_process: i32,
    pub system: i32,
    pub timeout: i32,
    pub bad_ssl: i32,
    pub canceled: i32,
}

/// What upstream's `start` accept loop does to its server and listening socket,
/// in its order; each callback gets `data` back.
#[repr(C)]
pub struct AcceptHost {
    pub data: *mut c_void,
    /// `hs->running`.
    pub running: unsafe extern "C" fn(*mut c_void) -> bool,
    /// `sock->accept()` and, for a connection, the `onAccept` coroutine's cid.
    pub accept: unsafe extern "C" fn(*mut c_void, *mut bool) -> i64,
    /// The socket's `errCode`, and its NUL-terminated `errMsg`.
    pub error: unsafe extern "C" fn(*mut c_void, *mut *const c_char) -> i32,
    /// `System::sleep(SW_ACCEPT_RETRY_TIME)`.
    pub sleep: unsafe extern "C" fn(*mut c_void),
    /// `http_server_coro_set_error`.
    pub set_error: unsafe extern "C" fn(*mut c_void),
    /// The `E_WARNING` for a failed accept; the host then stops the server.
    pub fail: unsafe extern "C" fn(*mut c_void, Text),
}

/// Upstream's `start` loop: accept until the server stops. 1 is `RETURN_TRUE`,
/// 0 `RETURN_FALSE` after a failed accept, -1 a null argument.
///
/// # Safety
/// `host` and `errors` are null or valid for the call, and each callback may be
/// called with `host.data`; `error` leaves a NUL-terminated message.
#[must_use]
pub unsafe fn swr_v1_httpbindings_accept_loop(
    host: *const AcceptHost,
    errors: *const AcceptErrors,
) -> i32 {
    // SAFETY: `host` is null or valid for the call.
    let Some(host) = (unsafe { host.as_ref() }) else {
        return -1;
    };
    // SAFETY: `errors` is null or valid for the call.
    let Some(errors) = (unsafe { errors.as_ref() }) else {
        return -1;
    };
    let data = host.data;
    // SAFETY: the host's callbacks take its data (as for each call below).
    while unsafe { (host.running)(data) } {
        let mut connected = false;
        // SAFETY: as above, and a local output.
        let cid = unsafe { (host.accept)(data, &raw mut connected) };
        if connected {
            if cid < 0 {
                // SAFETY: as above.
                unsafe { (host.sleep)(data) };
            }
            continue;
        }
        let mut message = core::ptr::null();
        // SAFETY: as above, and a local output.
        let error = unsafe { (host.error)(data, &raw mut message) };
        if error == errors.per_process || error == errors.system {
            // SAFETY: as above.
            unsafe { (host.sleep)(data) };
        } else if error == errors.timeout || error == errors.bad_ssl {
        } else if error == errors.canceled {
            // SAFETY: as above.
            unsafe { (host.set_error)(data) };
            break;
        } else {
            // SAFETY: as above.
            unsafe { (host.set_error)(data) };
            // SAFETY: `error` left the socket's NUL-terminated message.
            let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
            let warning = format!("accept failed, Error: {message}[{error}]");
            let text = Text {
                data: warning.as_ptr(),
                length: warning.len(),
            };
            // SAFETY: as above; `text` spans `warning`, live for the call.
            unsafe { (host.fail)(data, text) };
            return 0;
        }
    }
    1
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

    fn script<'a>(data: *mut c_void) -> &'a mut Script {
        // SAFETY: `run` passes its own live `Script`.
        unsafe { &mut *data.cast::<Script>() }
    }
    unsafe extern "C" fn running(data: *mut c_void) -> bool {
        !script(data).rounds.is_empty()
    }
    unsafe extern "C" fn accept(data: *mut c_void, connected: *mut bool) -> i64 {
        let script = script(data);
        script.trace.push("accept".into());
        let (connection, cid, _) = script.rounds[0];
        if connection {
            script.rounds.remove(0);
        }
        // SAFETY: the loop's local output.
        unsafe { *connected = connection };
        cid
    }
    unsafe extern "C" fn error(data: *mut c_void, message: *mut *const c_char) -> i32 {
        // SAFETY: the loop's local output.
        unsafe { *message = c"boom".as_ptr() };
        script(data).rounds.remove(0).2
    }
    unsafe extern "C" fn sleep(data: *mut c_void) {
        script(data).trace.push("sleep".into());
    }
    unsafe extern "C" fn set_error(data: *mut c_void) {
        script(data).trace.push("set-error".into());
    }
    unsafe extern "C" fn fail(data: *mut c_void, text: Text) {
        // SAFETY: the loop lends the warning's bytes for the call.
        let text = unsafe { core::slice::from_raw_parts(text.data, text.length) };
        script(data)
            .trace
            .push(String::from_utf8_lossy(text).into());
    }

    fn run(rounds: &[(bool, i64, i32)]) -> (i32, Vec<String>) {
        let mut script = Script {
            rounds: rounds.to_vec(),
            trace: Vec::new(),
        };
        let host = AcceptHost {
            data: (&raw mut script).cast(),
            running,
            accept,
            error,
            sleep,
            set_error,
            fail,
        };
        // SAFETY: a live host and policy table.
        let result = unsafe { swr_v1_httpbindings_accept_loop(&raw const host, &raw const ERRORS) };
        (result, script.trace)
    }

    fn trace(events: &[&str]) -> Vec<String> {
        events.iter().map(|event| (*event).to_owned()).collect()
    }

    #[test]
    fn accept_loop_follows_upstreams_branches() {
        assert_eq!(
            run(&[]),
            (1, trace(&[])),
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
            (1, trace(&[&expected[..], &["accept", "accept"]].concat())),
            "a failed coroutine and EMFILE/ENFILE sleep; timeouts and bad TLS clients retry"
        );
        assert_eq!(
            run(&[(false, 0, 125), (true, 1, 0)]),
            (1, trace(&["accept", "set-error"])),
            "a cancel records the error and stops accepting"
        );
        assert_eq!(
            run(&[(false, 0, 9), (true, 1, 0)]),
            (
                0,
                trace(&["accept", "set-error", "accept failed, Error: boom[9]"])
            ),
            "any other error records it, warns and returns false"
        );
        // SAFETY: a null host is refused before any use.
        let null = unsafe { swr_v1_httpbindings_accept_loop(core::ptr::null(), &raw const ERRORS) };
        assert_eq!(null, -1, "a null host is refused");
    }
}
