//! `HttpProxy`'s create, pack and handshake over corebase's proxy owner, which the
//! clients and hooks call, over crates/http's CONNECT formatting.
use crate::{Handshake, HttpProxyRequest, proxy_authorization, proxy_handshake, proxy_pack};
use swoole_rs_corebase::{ProxyState, VERSION, log, set_errno};

const HANDSHAKE_FAILED: i32 =
    swoole_rs_error::ErrorCode::SW_ERROR_HTTP_PROXY_HANDSHAKE_FAILED as i32;

/// `HttpProxy::create`'s assignments on a fresh owner: the host and port, then the
/// credentials when both are set. Only an allocation can fail, and that aborts as
/// Rust's allocation failures do.
#[must_use]
pub fn http_proxy(host: &[u8], port: i32, user: &[u8], password: &[u8]) -> Box<ProxyState> {
    let proxy = ProxyState::new(false);
    let authenticated = !user.is_empty() && !password.is_empty();
    if proxy
        .configure([host, user, password], port, authenticated, -1)
        .is_err()
    {
        swoole_rs_corebase::fatal("the HTTP proxy settings can't be stored");
    }
    proxy
}

/// `HttpProxy::get_auth_str()`: the Base64 credentials, errno EOVERFLOW when they
/// were cut to fit.
#[must_use]
pub fn authorization(owner: &ProxyState) -> Vec<u8> {
    let mut encoded = [0u8; 512];
    let (status, length) = proxy_authorization(&owner.string(1), &owner.string(2), &mut encoded);
    if status == 1 {
        set_errno(libc::EOVERFLOW);
    }
    let encoded = encoded.get(..length).unwrap_or_default();
    encoded
        .split(|&b| b == 0)
        .next()
        .unwrap_or_default()
        .to_vec()
}

/// `HttpProxy::pack`: the CONNECT request for `host` into `output`, NUL-terminated
/// and cut to fit, with the authorization when the proxy has a password. Returns
/// the bytes written (the whole length for an empty output).
pub fn proxy_connect(state: &ProxyState, host: &[u8], output: &mut [u8]) -> usize {
    let authenticated = !state.string(2).is_empty();
    let auth = if authenticated {
        authorization(state)
    } else {
        Vec::new()
    };
    let request = HttpProxyRequest {
        target: &state.string(3),
        host,
        version: VERSION.to_bytes(),
        authorization: &auth,
        port: state.fields().target_port,
        authenticated,
    };
    let Ok(text) = proxy_pack(&request) else {
        return 0;
    };
    if output.is_empty() {
        return text.len();
    }
    if output.len() > i32::MAX as usize {
        return 0;
    }
    let length = text.len().min(output.len() - 1);
    let (Some(to), Some(from)) = (output.get_mut(..length), text.get(..length)) else {
        return 0;
    };
    to.copy_from_slice(from);
    if let Some(end) = output.get_mut(length) {
        *end = 0;
    }
    length
}

/// `HttpProxy::handshake`: whether `input` accepts the CONNECT, setting the last
/// error when the proxy refused it.
#[must_use]
pub fn proxy_accept(input: &[u8]) -> bool {
    let status = proxy_handshake(input);
    if status == Handshake::Rejected {
        log::set_last_error(HANDSHAKE_FAILED);
    }
    status == Handshake::Accepted
}

/// Why the CONNECT response could not be read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProxyReadError {
    Socket,
    BadResponse(&'static str),
}

impl ProxyReadError {
    /// The synchronous, asynchronous or coroutine client's socket error.
    #[must_use]
    pub const fn code(self, socket_error: swoole_rs_error::ErrorCode) -> i32 {
        match self {
            Self::Socket => socket_error as i32,
            Self::BadResponse(_) => {
                swoole_rs_error::ErrorCode::SW_ERROR_HTTP_PROXY_BAD_RESPONSE as i32
            }
        }
    }
}

/// Peek at the next proxy response fragment, consuming only bytes belonging to the header.
/// Returns false if a further read is needed.
///
/// # Errors
/// Socket I/O failed or the bounded response is invalid.
pub fn http_proxy_fragment(
    input: &mut swoole_rs_corebase::buffer::string::StringBuffer,
    mut read: impl FnMut(
        &mut swoole_rs_corebase::lent::Spare<'_>,
        bool,
    ) -> Result<Option<usize>, ProxyReadError>,
) -> Result<bool, ProxyReadError> {
    let start = input.len();
    if start == input.size() {
        return Err(ProxyReadError::BadResponse(
            "HTTP proxy response header is too large",
        ));
    }
    let Some(peeked) = input.fill(|room| read(room, true))? else {
        return Ok(false);
    };
    if peeked == 0 {
        return Err(ProxyReadError::BadResponse(
            "connection closed before the HTTP proxy response was complete",
        ));
    }
    let end = crate::proxy::proxy_response_length(input.view())
        .map_err(|_| ProxyReadError::BadResponse("invalid HTTP proxy response"))?;
    let consume = end
        .unwrap_or(input.len())
        .checked_sub(start)
        .ok_or(ProxyReadError::Socket)?;
    input.truncate(start);
    if input.fill_within(consume, |room| read(room, false))? != Some(consume) {
        return Err(ProxyReadError::Socket);
    }
    Ok(end.is_some())
}
