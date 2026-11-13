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

use std::{
    ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong},
    marker::PhantomData,
    ptr::{self, NonNull},
};
use swoole_rs_ssh2_sys::{
    KbdintResponse, LIBSSH2_AGENT, LIBSSH2_CHANNEL, LIBSSH2_ERROR_EAGAIN,
    LIBSSH2_ERROR_SOCKET_NONE, LIBSSH2_LISTENER, LIBSSH2_SESSION, LIBSSH2_SESSION_BLOCK_OUTBOUND,
    LIBSSH2_SFTP, LIBSSH2_SFTP_ATTRIBUTES, LIBSSH2_SFTP_HANDLE, PasswdChangeReq,
    libssh2_agent_connect, libssh2_agent_disconnect, libssh2_agent_get_identity,
    libssh2_agent_list_identities, libssh2_agent_publickey, libssh2_agent_userauth,
    libssh2_channel_close, libssh2_channel_direct_tcpip_ex, libssh2_channel_eof,
    libssh2_channel_flush_ex, libssh2_channel_forward_accept, libssh2_channel_forward_cancel,
    libssh2_channel_forward_listen_ex, libssh2_channel_get_exit_status, libssh2_channel_open_ex,
    libssh2_channel_process_startup, libssh2_channel_read_ex, libssh2_channel_request_pty_ex,
    libssh2_channel_request_pty_size_ex, libssh2_channel_send_eof, libssh2_channel_setenv_ex,
    libssh2_channel_write_ex, libssh2_scp_recv, libssh2_scp_send_ex, libssh2_session_disconnect_ex,
    libssh2_session_handshake, libssh2_sftp_close_handle, libssh2_sftp_fstat_ex, libssh2_sftp_init,
    libssh2_sftp_mkdir_ex, libssh2_sftp_open_ex, libssh2_sftp_read, libssh2_sftp_readdir_ex,
    libssh2_sftp_rename_ex, libssh2_sftp_rmdir_ex, libssh2_sftp_stat_ex, libssh2_sftp_symlink_ex,
    libssh2_sftp_tell, libssh2_sftp_unlink_ex, libssh2_sftp_write, libssh2_userauth_authenticated,
    libssh2_userauth_hostbased_fromfile_ex, libssh2_userauth_keyboard_interactive_ex,
    libssh2_userauth_list, libssh2_userauth_password_ex, libssh2_userauth_publickey_fromfile_ex,
    libssh2_userauth_publickey_frommemory,
};

/// `SW_EVENT_READ` and `SW_EVENT_WRITE` (crates/ffi checks them).
pub const READ: c_int = 1 << 9;
pub const WRITE: c_int = 1 << 10;

/// `swr_v1_ssh2_ops`: what `swr_v1_ssh2_bound` lends.
#[repr(C)]
pub struct Ops {
    /// Park the coroutine until `event` on the session's socket: false if
    /// the wait failed (`Socket::poll`).
    wait: unsafe extern "C" fn(session: *mut LIBSSH2_SESSION, event: c_int) -> bool,
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

/// A NUL-terminated string live for `'a`, as an alias takes the SDK's `const char *`
/// arguments it measures. A `const char *`'s layout, and made as [`Session`] is.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Text<'a>(NonNull<c_char>, PhantomData<&'a CStr>);

impl Text<'_> {
    /// The string, as libssh2 takes it.
    fn as_ptr(self) -> *const c_char {
        self.0.as_ptr()
    }

    /// `(unsigned int) strlen(text)`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the upstream alias casts strlen to unsigned int"
    )]
    fn len(self) -> c_uint {
        // SAFETY: the type's invariant: NUL-terminated and live for `'a`.
        unsafe { CStr::from_ptr(self.0.as_ptr()) }.count_bytes() as c_uint
    }
}

// libssh2's own declarations of two calls that read a live session's state and
// nothing else, with the session typed as the handle it is.
unsafe extern "C" {
    #[link_name = "libssh2_session_block_directions"]
    safe fn block_directions(session: Session) -> c_int;
    #[link_name = "libssh2_session_last_errno"]
    safe fn session_last_errno(session: Session) -> c_int;
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
        // SAFETY: the lent table's wait, on the session it can wait on (the
        // export's contract).
        unsafe { (self.ops.wait)(self.session.0, event) }
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
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's std::function<int(void)>"
    )]
    fn narrow(self) -> c_int {
        self as c_int
    }
}

impl Narrow for usize {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "upstream's std::function<int(void)>"
    )]
    fn narrow(self) -> c_int {
        self as c_int
    }
}

/// One export per hooked libssh2 function.
macro_rules! hooks {
    ($($dispatch:ident $kind:ident $export:ident($($arg:ident: $ty:ty),*) -> $ret:ty => $function:ident;)*) => {$(
        hooks!(@$kind $export($($arg: $ty),*) -> $ret => $function);
    )*
        /// C call-site forwarding, derived from the same typed Rust hook inventory.
        pub const NATIVE_DISPATCH: &str = concat!($(hooks!(@dispatch $dispatch $function, $export($($arg),*))),*);
    };
    (@params $first:ident $(, $rest:ident)*) => {
        concat!(stringify!($first), $(", ", stringify!($rest),)*)
    };
    (@dispatch internal $function:ident, $export:ident($($arg:ident),*)) => { "" };
    (@dispatch direct $function:ident, $export:ident($($arg:ident),*)) => {
        concat!("#undef ", stringify!($function), "\n#define ", stringify!($function), "(", hooks!(@params $($arg),*), ") ",
                stringify!($export), "(ssh2_bound(session), session", $(", ", stringify!($arg),)* ")\n")
    };
    (@int $export:ident($($arg:ident: $ty:ty),*) -> $ret:ty => $function:ident) => {
        #[doc = concat!("`", stringify!($function), "`, retried while it says `EAGAIN`.")]
        /// # Safety
        /// `ops` is the lent table, `wait_on` a live session it can wait on, and
        /// the rest are the libssh2 function's arguments.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $export(ops: &Ops, wait_on: Session, $($arg: $ty),*) -> $ret {
            let wait = Wait { ops, session: wait_on };
            // SAFETY: the caller's arguments, as libssh2 takes them.
            wait.retry(wait.blocked(), || unsafe { $function($($arg),*) }.narrow())
        }
    };
    (@pointer $export:ident($($arg:ident: $ty:ty),*) -> $ret:ty => $function:ident) => {
        #[doc = concat!("`", stringify!($function), "`, retried while it fails with `EAGAIN`.")]
        /// # Safety
        /// As the `int` exports.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $export(ops: &Ops, wait_on: Session, $($arg: $ty),*) -> $ret {
            let wait = Wait { ops, session: wait_on };
            // SAFETY: the caller's arguments, as libssh2 takes them.
            let call = || unsafe { $function($($arg),*) };
            wait.retry_pointer(wait.blocked(), call, || wait.last_errno())
        }
    };
}

hooks! {
    direct int swoole_libssh2_session_handshake(session: *mut LIBSSH2_SESSION, sock: c_int) -> c_int => libssh2_session_handshake;
    internal int swoole_libssh2_session_disconnect_ex(session: *mut LIBSSH2_SESSION, reason: c_int, description: *const c_char, lang: *const c_char) -> c_int => libssh2_session_disconnect_ex;
    internal pointer swoole_libssh2_channel_open_ex(session: *mut LIBSSH2_SESSION, channel_type: *const c_char, channel_type_len: c_uint, window_size: c_uint, packet_size: c_uint, message: *const c_char, message_len: c_uint) -> *mut LIBSSH2_CHANNEL => libssh2_channel_open_ex;
    direct int swoole_libssh2_channel_setenv_ex(channel: *mut LIBSSH2_CHANNEL, varname: *const c_char, varname_len: c_uint, value: *const c_char, value_len: c_uint) -> c_int => libssh2_channel_setenv_ex;
    direct int swoole_libssh2_channel_request_pty_ex(channel: *mut LIBSSH2_CHANNEL, term: *const c_char, term_len: c_uint, modes: *const c_char, modes_len: c_uint, width: c_int, height: c_int, width_px: c_int, height_px: c_int) -> c_int => libssh2_channel_request_pty_ex;
    internal int swoole_libssh2_channel_process_startup(channel: *mut LIBSSH2_CHANNEL, request: *const c_char, request_len: c_uint, message: *const c_char, message_len: c_uint) -> c_int => libssh2_channel_process_startup;
    direct int swoole_libssh2_channel_flush_ex(channel: *mut LIBSSH2_CHANNEL, streamid: c_int) -> c_int => libssh2_channel_flush_ex;
    direct int swoole_libssh2_channel_read_ex(channel: *mut LIBSSH2_CHANNEL, stream_id: c_int, buf: *mut c_char, buflen: usize) -> c_int => libssh2_channel_read_ex;
    direct int swoole_libssh2_channel_write_ex(channel: *mut LIBSSH2_CHANNEL, stream_id: c_int, buf: *const c_char, buflen: usize) -> c_int => libssh2_channel_write_ex;
    direct int swoole_libssh2_channel_eof(channel: *mut LIBSSH2_CHANNEL) -> c_int => libssh2_channel_eof;
    direct int swoole_libssh2_channel_close(channel: *mut LIBSSH2_CHANNEL) -> c_int => libssh2_channel_close;
    direct int swoole_libssh2_channel_send_eof(channel: *mut LIBSSH2_CHANNEL) -> c_int => libssh2_channel_send_eof;
    direct int swoole_libssh2_channel_get_exit_status(channel: *mut LIBSSH2_CHANNEL) -> c_int => libssh2_channel_get_exit_status;
    direct int swoole_libssh2_channel_request_pty_size_ex(channel: *mut LIBSSH2_CHANNEL, width: c_int, height: c_int, width_px: c_int, height_px: c_int) -> c_int => libssh2_channel_request_pty_size_ex;
    direct pointer swoole_libssh2_channel_forward_listen_ex(session: *mut LIBSSH2_SESSION, host: *const c_char, port: c_int, bound_port: *mut c_int, queue_maxsize: c_int) -> *mut LIBSSH2_LISTENER => libssh2_channel_forward_listen_ex;
    direct pointer swoole_libssh2_channel_forward_accept(listener: *mut LIBSSH2_LISTENER) -> *mut LIBSSH2_CHANNEL => libssh2_channel_forward_accept;
    direct int swoole_libssh2_channel_forward_cancel(listener: *mut LIBSSH2_LISTENER) -> c_int => libssh2_channel_forward_cancel;
    internal pointer swoole_libssh2_channel_direct_tcpip_ex(session: *mut LIBSSH2_SESSION, host: *const c_char, port: c_int, shost: *const c_char, sport: c_int) -> *mut LIBSSH2_CHANNEL => libssh2_channel_direct_tcpip_ex;
    internal int swoole_libssh2_sftp_fstat_ex(handle: *mut LIBSSH2_SFTP_HANDLE, attrs: *mut LIBSSH2_SFTP_ATTRIBUTES, setstat: c_int) -> c_int => libssh2_sftp_fstat_ex;
    direct int swoole_libssh2_sftp_stat_ex(sftp: *mut LIBSSH2_SFTP, path: *const c_char, path_len: c_uint, stat_type: c_int, attrs: *mut LIBSSH2_SFTP_ATTRIBUTES) -> c_int => libssh2_sftp_stat_ex;
    direct int swoole_libssh2_sftp_symlink_ex(sftp: *mut LIBSSH2_SFTP, path: *const c_char, path_len: c_uint, target: *mut c_char, target_len: c_uint, link_type: c_int) -> c_int => libssh2_sftp_symlink_ex;
    internal pointer swoole_libssh2_sftp_open_ex(sftp: *mut LIBSSH2_SFTP, filename: *const c_char, filename_len: c_uint, flags: c_ulong, mode: c_long, open_type: c_int) -> *mut LIBSSH2_SFTP_HANDLE => libssh2_sftp_open_ex;
    internal int swoole_libssh2_sftp_readdir_ex(handle: *mut LIBSSH2_SFTP_HANDLE, buffer: *mut c_char, buffer_maxlen: usize, longentry: *mut c_char, longentry_maxlen: usize, attrs: *mut LIBSSH2_SFTP_ATTRIBUTES) -> c_int => libssh2_sftp_readdir_ex;
    direct int swoole_libssh2_sftp_tell(handle: *mut LIBSSH2_SFTP_HANDLE) -> c_int => libssh2_sftp_tell;
    direct int swoole_libssh2_sftp_read(handle: *mut LIBSSH2_SFTP_HANDLE, buffer: *mut c_char, buffer_maxlen: usize) -> c_int => libssh2_sftp_read;
    direct int swoole_libssh2_sftp_write(handle: *mut LIBSSH2_SFTP_HANDLE, buffer: *const c_char, count: usize) -> c_int => libssh2_sftp_write;
    direct pointer swoole_libssh2_sftp_init(session: *mut LIBSSH2_SESSION) -> *mut LIBSSH2_SFTP => libssh2_sftp_init;
    direct pointer swoole_libssh2_scp_recv(session: *mut LIBSSH2_SESSION, path: *const c_char, sb: *mut libc::stat) -> *mut LIBSSH2_CHANNEL => libssh2_scp_recv;
    direct pointer swoole_libssh2_scp_send_ex(session: *mut LIBSSH2_SESSION, path: *const c_char, mode: c_int, size: usize, mtime: c_long, atime: c_long) -> *mut LIBSSH2_CHANNEL => libssh2_scp_send_ex;
    internal int swoole_libssh2_sftp_close_handle(handle: *mut LIBSSH2_SFTP_HANDLE) -> c_int => libssh2_sftp_close_handle;
    direct int swoole_libssh2_sftp_unlink_ex(sftp: *mut LIBSSH2_SFTP, filename: *const c_char, filename_len: c_uint) -> c_int => libssh2_sftp_unlink_ex;
    direct int swoole_libssh2_sftp_rename_ex(sftp: *mut LIBSSH2_SFTP, source_filename: *const c_char, source_filename_len: c_uint, dest_filename: *const c_char, dest_filename_len: c_uint, flags: c_long) -> c_int => libssh2_sftp_rename_ex;
    direct int swoole_libssh2_sftp_mkdir_ex(sftp: *mut LIBSSH2_SFTP, path: *const c_char, path_len: c_uint, mode: c_long) -> c_int => libssh2_sftp_mkdir_ex;
    direct int swoole_libssh2_sftp_rmdir_ex(sftp: *mut LIBSSH2_SFTP, path: *const c_char, path_len: c_uint) -> c_int => libssh2_sftp_rmdir_ex;
    direct int swoole_libssh2_agent_connect(agent: *mut LIBSSH2_AGENT) -> c_int => libssh2_agent_connect;
    direct int swoole_libssh2_agent_list_identities(agent: *mut LIBSSH2_AGENT) -> c_int => libssh2_agent_list_identities;
    direct int swoole_libssh2_agent_get_identity(agent: *mut LIBSSH2_AGENT, store: *mut *mut libssh2_agent_publickey, prev: *mut libssh2_agent_publickey) -> c_int => libssh2_agent_get_identity;
    direct int swoole_libssh2_agent_userauth(agent: *mut LIBSSH2_AGENT, username: *const c_char, identity: *mut libssh2_agent_publickey) -> c_int => libssh2_agent_userauth;
    direct int swoole_libssh2_agent_disconnect(agent: *mut LIBSSH2_AGENT) -> c_int => libssh2_agent_disconnect;
    direct pointer swoole_libssh2_userauth_list(session: *mut LIBSSH2_SESSION, username: *const c_char, username_len: c_uint) -> *mut c_char => libssh2_userauth_list;
    direct int swoole_libssh2_userauth_password_ex(session: *mut LIBSSH2_SESSION, username: *const c_char, username_len: c_uint, password: *const c_char, password_len: c_uint, passwd_change_cb: Option<PasswdChangeReq>) -> c_int => libssh2_userauth_password_ex;
    direct int swoole_libssh2_userauth_publickey_fromfile_ex(session: *mut LIBSSH2_SESSION, username: *const c_char, username_len: c_uint, publickey: *const c_char, privatekey: *const c_char, passphrase: *const c_char) -> c_int => libssh2_userauth_publickey_fromfile_ex;
    direct int swoole_libssh2_userauth_publickey_frommemory(session: *mut LIBSSH2_SESSION, username: *const c_char, username_len: usize, publickeyfiledata: *const c_char, publickeyfiledata_len: usize, privatekeyfiledata: *const c_char, privatekeyfiledata_len: usize, passphrase: *const c_char) -> c_int => libssh2_userauth_publickey_frommemory;
    direct int swoole_libssh2_userauth_hostbased_fromfile_ex(session: *mut LIBSSH2_SESSION, username: *const c_char, username_len: c_uint, publickey: *const c_char, privatekey: *const c_char, passphrase: *const c_char, hostname: *const c_char, hostname_len: c_uint, local_username: *const c_char, local_username_len: c_uint) -> c_int => libssh2_userauth_hostbased_fromfile_ex;
    internal int swoole_libssh2_userauth_keyboard_interactive_ex(session: *mut LIBSSH2_SESSION, username: *const c_char, username_len: c_uint, response_callback: Option<KbdintResponse>) -> c_int => libssh2_userauth_keyboard_interactive_ex;
    direct int swoole_libssh2_userauth_authenticated(session: *mut LIBSSH2_SESSION) -> c_int => libssh2_userauth_authenticated;
}

#[cfg(test)]
mod tests;

pub mod aliases;
pub use aliases::DISPATCH as ALIAS_DISPATCH;
