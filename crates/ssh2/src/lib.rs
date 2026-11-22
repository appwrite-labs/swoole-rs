//! Swoole's coroutine ssh2 hook (upstream's `php_swoole_ssh2.h`
//! `ssh2_async_call`, built with `--with-swoole-ssh2`). The bundled
//! `ext/ssh2` runs its libssh2 session non-blocking; `php_swoole_ssh2_hook.h`
//! sends each blocking libssh2 call here, which repeats it while libssh2 says
//! `EAGAIN`, parking the coroutine on the session's socket in between.
//!
//! `swr_v1_ssh2_bound` (crates/hooks) checks that no other coroutine is bound
//! to the socket before each call and lends the wait ([`Ops`]). Each export
//! is `swoole_<libssh2 function>`, taking those operations and the session to
//! wait on before the function's own arguments.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

#[cfg(feature = "php")]
mod php;

use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong};
use std::ptr;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::sys::Text;
use swoole_rs_ssh2_sys::{
    KbdintResponse, LIBSSH2_AGENT, LIBSSH2_CHANNEL, LIBSSH2_ERROR_EAGAIN,
    LIBSSH2_ERROR_SOCKET_NONE, LIBSSH2_LISTENER, LIBSSH2_SESSION, LIBSSH2_SESSION_BLOCK_OUTBOUND,
    LIBSSH2_SFTP, LIBSSH2_SFTP_ATTRIBUTES, LIBSSH2_SFTP_HANDLE, PasswdChangeReq,
    libssh2_agent_connect, libssh2_agent_disconnect, libssh2_agent_get_identity,
    libssh2_agent_list_identities, libssh2_agent_publickey, libssh2_agent_userauth,
    libssh2_channel_close, libssh2_channel_eof, libssh2_channel_flush_ex,
    libssh2_channel_forward_accept, libssh2_channel_forward_cancel,
    libssh2_channel_forward_listen_ex, libssh2_channel_get_exit_status, libssh2_channel_read_ex,
    libssh2_channel_request_pty_ex, libssh2_channel_request_pty_size_ex, libssh2_channel_send_eof,
    libssh2_channel_setenv_ex, libssh2_channel_write_ex, libssh2_scp_recv, libssh2_scp_send_ex,
    libssh2_session_handshake, libssh2_sftp_init, libssh2_sftp_mkdir_ex, libssh2_sftp_read,
    libssh2_sftp_rename_ex, libssh2_sftp_rmdir_ex, libssh2_sftp_stat_ex, libssh2_sftp_symlink_ex,
    libssh2_sftp_tell, libssh2_sftp_unlink_ex, libssh2_sftp_write, libssh2_userauth_authenticated,
    libssh2_userauth_hostbased_fromfile_ex, libssh2_userauth_list, libssh2_userauth_password_ex,
    libssh2_userauth_publickey_fromfile_ex, libssh2_userauth_publickey_frommemory,
};

/// `SW_EVENT_READ` and `SW_EVENT_WRITE` (crates/ffi checks them).
pub const READ: c_int = 1 << 9;
pub const WRITE: c_int = 1 << 10;

/// `swr_v1_ssh2_ops`: what `swr_v1_ssh2_bound` lends, in a coroutine, for the session
/// it was asked about. Only an export's parameter lends one (its field is private),
/// with that session as the export's [`Session`]. One function pointer, so the C
/// struct's layout.
#[repr(transparent)]
pub struct Ops {
    /// Park the coroutine until `event` on the session's socket: false if
    /// the wait failed (`Socket::poll`). The lender's function, whose contract
    /// (a live ext/ssh2 session, in a coroutine) the lent table and session carry.
    wait: extern "C" fn(session: Session, event: c_int) -> bool,
}

/// The session's socket event libssh2 is blocked on (`ssh2_get_event_type`).
fn event(directions: c_int) -> c_int {
    if directions & LIBSSH2_SESSION_BLOCK_OUTBOUND != 0 {
        WRITE
    } else {
        READ
    }
}

/// The session an export parks on, live for as long as the value is used. Only an
/// export's parameter makes one, where the C caller's contract (the export's
/// `# Safety`) stands for the constructor, as it does for a `&T` parameter there.
/// A `LIBSSH2_SESSION *`'s layout, and neither `Send` nor `Sync`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Session(*mut LIBSSH2_SESSION);

/// A channel, SFTP session or SFTP handle an alias was lent, live for as long as
/// the value is used: made as [`Session`] is, with the layout of the pointer it holds.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Channel(*mut LIBSSH2_CHANNEL);
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Sftp(*mut LIBSSH2_SFTP);
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct SftpHandle(*mut LIBSSH2_SFTP_HANDLE);

/// `(unsigned int) strlen(text)`, as the upstream aliases measure their strings.
/// The upstream alias casts strlen to unsigned int.
fn length(text: &CStr) -> c_uint {
    text.count_bytes().c_cast::<c_uint>()
}

// libssh2's own declarations, with each handle typed as the live one it is. The
// first two read a session's state and nothing else. The others take only live
// handles, strings libssh2 reads up to their NUL, the attributes record it fills or
// reads, and plain values; they drive that session alone, free nothing, and run no
// callback.
unsafe extern "C" {
    #[link_name = "libssh2_session_block_directions"]
    safe fn block_directions(session: Session) -> c_int;
    #[link_name = "libssh2_session_last_errno"]
    safe fn session_last_errno(session: Session) -> c_int;
    #[link_name = "libssh2_session_disconnect_ex"]
    safe fn disconnect(
        session: Session,
        reason: c_int,
        description: Text<'_>,
        lang: Text<'_>,
    ) -> c_int;
    #[link_name = "libssh2_channel_direct_tcpip_ex"]
    safe fn direct_tcpip(
        session: Session,
        host: Text<'_>,
        port: c_int,
        shost: Text<'_>,
        sport: c_int,
    ) -> *mut LIBSSH2_CHANNEL;
    #[link_name = "libssh2_sftp_fstat_ex"]
    safe fn fstat(handle: SftpHandle, attrs: &mut LIBSSH2_SFTP_ATTRIBUTES, setstat: c_int)
    -> c_int;
    /// Opens a channel of the type `kind` names, which libssh2 reads for the length
    /// [`KindLength`] gives it, with no message ([`NoMessage`]).
    #[link_name = "libssh2_channel_open_ex"]
    safe fn channel_open(
        session: Session,
        kind: &[u8; 8],
        kind_length: KindLength,
        window_size: c_uint,
        packet_size: c_uint,
        message: Option<&c_char>,
        message_length: NoMessage,
    ) -> *mut LIBSSH2_CHANNEL;
}

/// The channel type length excludes its trailing NUL.
#[derive(Clone, Copy)]
#[repr(u32)]
enum KindLength {
    Session = 7,
}

/// `channel_open`'s message length: none.
#[derive(Clone, Copy)]
#[repr(u32)]
enum NoMessage {
    Empty = 0,
}

/// `libssh2_channel_open_session(session)`: a session channel with libssh2's default
/// window and packet sizes.
fn open_session(session: Session) -> *mut LIBSSH2_CHANNEL {
    channel_open(
        session,
        b"session\0",
        KindLength::Session,
        2 * 1024 * 1024,
        32768,
        None,
        NoMessage::Empty,
    )
}

/// The lent wait and the session it can wait on, as an export received them.
struct Wait<'a> {
    ops: &'a Ops,
    session: Session,
}

impl Wait<'_> {
    /// The event to wait for on the session.
    fn blocked(&self) -> c_int {
        event(block_directions(self.session))
    }

    /// The session's `last_errno`.
    fn last_errno(&self) -> c_int {
        session_last_errno(self.session)
    }

    /// Park the coroutine until `event` on the session's socket.
    fn park(&self, event: c_int) -> bool {
        (self.ops.wait)(self.session, event)
    }

    /// `call`, retried as its [`Answer`] says.
    fn run<R: Answer>(&self, call: impl FnMut() -> R) -> R::Result {
        R::retried(self, call)
    }

    /// [`retry`](Self::retry) from the event the session is blocked on now.
    fn call(&self, call: impl FnMut() -> c_int) -> c_int {
        self.retry(self.blocked(), call)
    }

    /// [`retry_pointer`](Self::retry_pointer) from the event the session is blocked
    /// on now, `EAGAIN` being its `last_errno`.
    fn call_pointer<T>(&self, call: impl FnMut() -> *mut T) -> *mut T {
        self.retry_pointer(self.blocked(), call, || self.last_errno())
    }

    /// `ssh2_async_call`: `call` until it stops saying `EAGAIN`, waiting for
    /// `event`, the direction libssh2 was blocked on when it started.
    fn retry(&self, event: c_int, mut call: impl FnMut() -> c_int) -> c_int {
        loop {
            let result = call();
            if result != LIBSSH2_ERROR_EAGAIN {
                return result;
            }
            if !self.park(event) {
                return LIBSSH2_ERROR_SOCKET_NONE;
            }
        }
    }

    /// `ssh2_async_call_ex`: as [`retry`](Self::retry), for a call that returns
    /// null when it fails, `EAGAIN` being the session's `last_errno`.
    fn retry_pointer<T>(
        &self,
        event: c_int,
        mut call: impl FnMut() -> *mut T,
        last_errno: impl Fn() -> c_int,
    ) -> *mut T {
        loop {
            let handle = call();
            if !handle.is_null() {
                return handle;
            }
            if last_errno() != LIBSSH2_ERROR_EAGAIN || !self.park(event) {
                return ptr::null_mut();
            }
        }
    }
}

/// An `int` result, as upstream's `std::function<int(void)>` narrows a
/// `ssize_t` or `size_t` one.
trait Narrow {
    fn narrow(self) -> c_int;
}

impl Narrow for c_int {
    fn narrow(self) -> c_int {
        self
    }
}

impl Narrow for isize {
    fn narrow(self) -> c_int {
        self.c_cast::<c_int>()
    }
}

impl Narrow for usize {
    fn narrow(self) -> c_int {
        self.c_cast::<c_int>()
    }
}

/// A hooked call's result, which says how the call is retried: an `int` one (narrowed
/// as [`Narrow`]) while it is `EAGAIN`, a handle while it is null with `EAGAIN` the
/// session's `last_errno`.
pub(crate) trait Answer: Sized {
    type Result;
    fn retried(wait: &Wait<'_>, call: impl FnMut() -> Self) -> Self::Result;
}

impl<N: Narrow> Answer for N {
    type Result = c_int;
    fn retried(wait: &Wait<'_>, mut call: impl FnMut() -> Self) -> Self::Result {
        wait.call(|| call().narrow())
    }
}

impl<T> Answer for *mut T {
    type Result = Self;
    fn retried(wait: &Wait<'_>, call: impl FnMut() -> Self) -> Self {
        wait.call_pointer(call)
    }
}

impl Answer for Option<Text<'_>> {
    type Result = Self;
    /// As a handle: retried while it is null with `EAGAIN` the session's
    /// `last_errno`, from the event the session is blocked on now.
    fn retried(wait: &Wait<'_>, mut call: impl FnMut() -> Self) -> Self {
        let event = wait.blocked();
        loop {
            let text = call();
            if text.is_some() {
                return text;
            }
            if wait.last_errno() != LIBSSH2_ERROR_EAGAIN || !wait.park(event) {
                return None;
            }
        }
    }
}

/// One export per hooked libssh2 function.
macro_rules! hooks {
    ($($export:ident($($arg:ident: $ty:ty),*) -> $ret:ty => $function:ident;)*) => {$(
        hooks!(@export $export($($arg: $ty),*) -> $ret => $function);
    )*
        /// C call-site forwarding, derived from the same typed Rust hook inventory.
        pub const NATIVE_DISPATCH: &str = concat!($(hooks!(@dispatch $function, $export($($arg),*))),*);
    };
    (@params $first:ident $(, $rest:ident)*) => {
        concat!(stringify!($first), $(", ", stringify!($rest),)*)
    };
    (@dispatch $function:ident, $export:ident($($arg:ident),*)) => {
        concat!("#undef ", stringify!($function), "\n#define ", stringify!($function), "(", hooks!(@params $($arg),*), ") ",
                stringify!($export), "(ssh2_bound(session), session", $(", ", stringify!($arg),)* ")\n")
    };
    (@export $export:ident($($arg:ident: $ty:ty),*) -> $ret:ty => $function:ident) => {
        hooks!(@define concat!("`", stringify!($function), "`, retried while it fails with `EAGAIN`."),
            ops, wait_on, $export($($arg: $ty),*) -> $ret = [$function($($arg),*)]);
    };
    // One export, its body a block over the safe helpers or, in brackets, the libssh2
    // call, retried as the hooked calls are.
    (@define $doc:expr, $ops:ident, $wait:ident, $export:ident($($arg:ident: $ty:ty),*) -> $ret:ty = $body:tt) => {
        #[doc = $doc]
        /// # Safety
        /// `ops` is the lent table, `wait_on` a live session it can wait on, and
        /// the rest are the libssh2 function's arguments.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $export($ops: &Ops, $wait: Session, $($arg: $ty),*) -> $ret {
            hooks!(@body $ops, $wait, $body)
        }
    };
    (@body $ops:ident, $wait:ident, { $($body:tt)* }) => {{ $($body)* }};
    (@body $ops:ident, $wait:ident, [ $function:ident($($call:expr),*) ]) => {
        // SAFETY: the caller's live handles and buffers, as libssh2 takes them; each
        // string libssh2 reads for its measured length, and each default a plain value
        // or a static string.
        Wait { ops: $ops, session: $wait }.run(|| unsafe { $function($($call),*) })
    };
}

hooks! {
    swoole_libssh2_session_handshake(session: *mut LIBSSH2_SESSION, sock: c_int) -> c_int => libssh2_session_handshake;
    swoole_libssh2_channel_setenv_ex(channel: *mut LIBSSH2_CHANNEL, varname: *const c_char, varname_len: c_uint, value: *const c_char, value_len: c_uint) -> c_int => libssh2_channel_setenv_ex;
    swoole_libssh2_channel_request_pty_ex(channel: *mut LIBSSH2_CHANNEL, term: *const c_char, term_len: c_uint, modes: *const c_char, modes_len: c_uint, width: c_int, height: c_int, width_px: c_int, height_px: c_int) -> c_int => libssh2_channel_request_pty_ex;
    swoole_libssh2_channel_flush_ex(channel: *mut LIBSSH2_CHANNEL, streamid: c_int) -> c_int => libssh2_channel_flush_ex;
    swoole_libssh2_channel_read_ex(channel: *mut LIBSSH2_CHANNEL, stream_id: c_int, buf: *mut c_char, buflen: usize) -> c_int => libssh2_channel_read_ex;
    swoole_libssh2_channel_write_ex(channel: *mut LIBSSH2_CHANNEL, stream_id: c_int, buf: *const c_char, buflen: usize) -> c_int => libssh2_channel_write_ex;
    swoole_libssh2_channel_eof(channel: *mut LIBSSH2_CHANNEL) -> c_int => libssh2_channel_eof;
    swoole_libssh2_channel_close(channel: *mut LIBSSH2_CHANNEL) -> c_int => libssh2_channel_close;
    swoole_libssh2_channel_send_eof(channel: *mut LIBSSH2_CHANNEL) -> c_int => libssh2_channel_send_eof;
    swoole_libssh2_channel_get_exit_status(channel: *mut LIBSSH2_CHANNEL) -> c_int => libssh2_channel_get_exit_status;
    swoole_libssh2_channel_request_pty_size_ex(channel: *mut LIBSSH2_CHANNEL, width: c_int, height: c_int, width_px: c_int, height_px: c_int) -> c_int => libssh2_channel_request_pty_size_ex;
    swoole_libssh2_channel_forward_listen_ex(session: *mut LIBSSH2_SESSION, host: *const c_char, port: c_int, bound_port: *mut c_int, queue_maxsize: c_int) -> *mut LIBSSH2_LISTENER => libssh2_channel_forward_listen_ex;
    swoole_libssh2_channel_forward_accept(listener: *mut LIBSSH2_LISTENER) -> *mut LIBSSH2_CHANNEL => libssh2_channel_forward_accept;
    swoole_libssh2_channel_forward_cancel(listener: *mut LIBSSH2_LISTENER) -> c_int => libssh2_channel_forward_cancel;
    swoole_libssh2_sftp_stat_ex(sftp: *mut LIBSSH2_SFTP, path: *const c_char, path_len: c_uint, stat_type: c_int, attrs: *mut LIBSSH2_SFTP_ATTRIBUTES) -> c_int => libssh2_sftp_stat_ex;
    swoole_libssh2_sftp_symlink_ex(sftp: *mut LIBSSH2_SFTP, path: *const c_char, path_len: c_uint, target: *mut c_char, target_len: c_uint, link_type: c_int) -> c_int => libssh2_sftp_symlink_ex;
    swoole_libssh2_sftp_tell(handle: *mut LIBSSH2_SFTP_HANDLE) -> c_int => libssh2_sftp_tell;
    swoole_libssh2_sftp_read(handle: *mut LIBSSH2_SFTP_HANDLE, buffer: *mut c_char, buffer_maxlen: usize) -> c_int => libssh2_sftp_read;
    swoole_libssh2_sftp_write(handle: *mut LIBSSH2_SFTP_HANDLE, buffer: *const c_char, count: usize) -> c_int => libssh2_sftp_write;
    swoole_libssh2_sftp_init(session: *mut LIBSSH2_SESSION) -> *mut LIBSSH2_SFTP => libssh2_sftp_init;
    swoole_libssh2_scp_recv(session: *mut LIBSSH2_SESSION, path: *const c_char, sb: *mut libc::stat) -> *mut LIBSSH2_CHANNEL => libssh2_scp_recv;
    swoole_libssh2_scp_send_ex(session: *mut LIBSSH2_SESSION, path: *const c_char, mode: c_int, size: usize, mtime: c_long, atime: c_long) -> *mut LIBSSH2_CHANNEL => libssh2_scp_send_ex;
    swoole_libssh2_sftp_unlink_ex(sftp: *mut LIBSSH2_SFTP, filename: *const c_char, filename_len: c_uint) -> c_int => libssh2_sftp_unlink_ex;
    swoole_libssh2_sftp_rename_ex(sftp: *mut LIBSSH2_SFTP, source_filename: *const c_char, source_filename_len: c_uint, dest_filename: *const c_char, dest_filename_len: c_uint, flags: c_long) -> c_int => libssh2_sftp_rename_ex;
    swoole_libssh2_sftp_mkdir_ex(sftp: *mut LIBSSH2_SFTP, path: *const c_char, path_len: c_uint, mode: c_long) -> c_int => libssh2_sftp_mkdir_ex;
    swoole_libssh2_sftp_rmdir_ex(sftp: *mut LIBSSH2_SFTP, path: *const c_char, path_len: c_uint) -> c_int => libssh2_sftp_rmdir_ex;
    swoole_libssh2_agent_connect(agent: *mut LIBSSH2_AGENT) -> c_int => libssh2_agent_connect;
    swoole_libssh2_agent_list_identities(agent: *mut LIBSSH2_AGENT) -> c_int => libssh2_agent_list_identities;
    swoole_libssh2_agent_get_identity(agent: *mut LIBSSH2_AGENT, store: *mut *mut libssh2_agent_publickey, prev: *mut libssh2_agent_publickey) -> c_int => libssh2_agent_get_identity;
    swoole_libssh2_agent_userauth(agent: *mut LIBSSH2_AGENT, username: *const c_char, identity: *mut libssh2_agent_publickey) -> c_int => libssh2_agent_userauth;
    swoole_libssh2_agent_disconnect(agent: *mut LIBSSH2_AGENT) -> c_int => libssh2_agent_disconnect;
    swoole_libssh2_userauth_list(session: *mut LIBSSH2_SESSION, username: *const c_char, username_len: c_uint) -> *mut c_char => libssh2_userauth_list;
    swoole_libssh2_userauth_password_ex(session: *mut LIBSSH2_SESSION, username: *const c_char, username_len: c_uint, password: *const c_char, password_len: c_uint, passwd_change_cb: Option<PasswdChangeReq>) -> c_int => libssh2_userauth_password_ex;
    swoole_libssh2_userauth_publickey_fromfile_ex(session: *mut LIBSSH2_SESSION, username: *const c_char, username_len: c_uint, publickey: *const c_char, privatekey: *const c_char, passphrase: *const c_char) -> c_int => libssh2_userauth_publickey_fromfile_ex;
    swoole_libssh2_userauth_publickey_frommemory(session: *mut LIBSSH2_SESSION, username: *const c_char, username_len: usize, publickeyfiledata: *const c_char, publickeyfiledata_len: usize, privatekeyfiledata: *const c_char, privatekeyfiledata_len: usize, passphrase: *const c_char) -> c_int => libssh2_userauth_publickey_frommemory;
    swoole_libssh2_userauth_hostbased_fromfile_ex(session: *mut LIBSSH2_SESSION, username: *const c_char, username_len: c_uint, publickey: *const c_char, privatekey: *const c_char, passphrase: *const c_char, hostname: *const c_char, hostname_len: c_uint, local_username: *const c_char, local_username_len: c_uint) -> c_int => libssh2_userauth_hostbased_fromfile_ex;
    swoole_libssh2_userauth_authenticated(session: *mut LIBSSH2_SESSION) -> c_int => libssh2_userauth_authenticated;
}

#[cfg(test)]
mod tests;

pub mod aliases;
pub use aliases::DISPATCH as ALIAS_DISPATCH;
