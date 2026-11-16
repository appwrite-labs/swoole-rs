//! Convenience/default argument policy for the original optional SDK aliases.
use super::{
    Channel, KbdintResponse, LIBSSH2_CHANNEL, LIBSSH2_SESSION, LIBSSH2_SFTP,
    LIBSSH2_SFTP_ATTRIBUTES, LIBSSH2_SFTP_HANDLE, Ops, Session, Sftp, SftpHandle, Text, Wait,
    c_char, c_int, c_long, c_ulong, direct_tcpip, disconnect, fstat, length, open_session, ptr,
};
use swoole_rs_ssh2_sys::{
    libssh2_channel_process_startup, libssh2_channel_read_ex, libssh2_channel_write_ex,
    libssh2_sftp_close_handle, libssh2_sftp_mkdir_ex, libssh2_sftp_open_ex,
    libssh2_sftp_readdir_ex, libssh2_sftp_rename_ex, libssh2_sftp_rmdir_ex, libssh2_sftp_unlink_ex,
    libssh2_userauth_keyboard_interactive_ex, libssh2_userauth_publickey_fromfile_ex,
};

/// One export per alias. Its body is a block over the safe helpers or, in brackets,
/// the canonical libssh2 call with the alias's defaults filled in, retried as the
/// hooked calls are.
macro_rules! aliases {
    ($($name:ident => $export:ident($ops:ident, $wait:ident; $($arg:ident: $ty:ty),*) -> $ret:ty = $body:tt)*) => {
        /// Native call-site forwarding; all argument policy executes in Rust.
        pub const DISPATCH: &str = concat!($(concat!("#undef ", stringify!($name), "\n#define ", stringify!($name),
            "(", hooks!(@params $($arg),*), ") ", stringify!($export), "(ssh2_bound(session), session", $(", ", stringify!($arg),)* ")\n")),*);
        $(
            #[doc = concat!("Original `", stringify!($name), "` convenience operation.")]
            /// # Safety
            /// The caller lends the original SDK arguments, the live session to wait on and
            /// the host table.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $export($ops: &Ops, $wait: Session, $($arg: $ty),*) -> $ret {
                aliases!(@body $ops, $wait, $body)
            }
        )*
    };
    (@body $ops:ident, $wait:ident, { $($body:tt)* }) => {{ $($body)* }};
    (@body $ops:ident, $wait:ident, [ $function:ident($($call:expr),*) ]) => {
        // SAFETY: the caller's live handles and buffers, as libssh2 takes them; each
        // string libssh2 reads for its measured length, and each default a plain value
        // or a static string.
        Wait { ops: $ops, session: $wait }.run(|| unsafe { $function($($call),*) })
    };
}

aliases! {
    libssh2_session_disconnect => swoole_libssh2_session_disconnect(ops, wait_on; session: Session, description: Text<'_>) -> c_int = {
        Wait { ops, session: wait_on }.call(|| disconnect(session, 11, description, c"".into()))
    }
    libssh2_channel_open_session => swoole_libssh2_channel_open_session(ops, wait_on; session: Session) -> *mut LIBSSH2_CHANNEL = {
        Wait { ops, session: wait_on }.call_pointer(|| open_session(session))
    }
    libssh2_channel_shell => swoole_libssh2_channel_shell(ops, wait_on; channel: Channel) -> c_int =
        [libssh2_channel_process_startup(channel.0, c"shell".as_ptr(), length(c"shell"), ptr::null(), 0)]
    libssh2_channel_exec => swoole_libssh2_channel_exec(ops, wait_on; channel: Channel, command: Text<'_>) -> c_int =
        [libssh2_channel_process_startup(channel.0, c"exec".as_ptr(), length(c"exec"), command.get().as_ptr(), length(command.get()))]
    libssh2_channel_read => swoole_libssh2_channel_read(ops, wait_on; channel: *mut LIBSSH2_CHANNEL, buf: *mut c_char, buflen: usize) -> c_int =
        [libssh2_channel_read_ex(channel, 0, buf, buflen)]
    libssh2_channel_write => swoole_libssh2_channel_write(ops, wait_on; channel: *mut LIBSSH2_CHANNEL, buf: *const c_char, buflen: usize) -> c_int =
        [libssh2_channel_write_ex(channel, 0, buf, buflen)]
    libssh2_channel_direct_tcpip => swoole_libssh2_channel_direct_tcpip(ops, wait_on; session: Session, host: Text<'_>, port: c_int) -> *mut LIBSSH2_CHANNEL = {
        Wait { ops, session: wait_on }.call_pointer(|| direct_tcpip(session, host, port, c"127.0.0.1".into(), 22))
    }
    libssh2_sftp_fstat => swoole_libssh2_sftp_fstat(ops, wait_on; handle: SftpHandle, attrs: &mut LIBSSH2_SFTP_ATTRIBUTES) -> c_int = {
        Wait { ops, session: wait_on }.call(|| fstat(handle, attrs, 0))
    }
    libssh2_sftp_open => swoole_libssh2_sftp_open(ops, wait_on; sftp: Sftp, filename: Text<'_>, flags: c_ulong, mode: c_long) -> *mut LIBSSH2_SFTP_HANDLE =
        [libssh2_sftp_open_ex(sftp.0, filename.get().as_ptr(), length(filename.get()), flags, mode, 0)]
    libssh2_sftp_opendir => swoole_libssh2_sftp_opendir(ops, wait_on; sftp: Sftp, path: Text<'_>) -> *mut LIBSSH2_SFTP_HANDLE =
        [libssh2_sftp_open_ex(sftp.0, path.get().as_ptr(), length(path.get()), 0, 0, 1)]
    libssh2_sftp_readdir => swoole_libssh2_sftp_readdir(ops, wait_on; handle: *mut LIBSSH2_SFTP_HANDLE, buffer: *mut c_char, buffer_maxlen: usize, attrs: *mut LIBSSH2_SFTP_ATTRIBUTES) -> c_int =
        [libssh2_sftp_readdir_ex(handle, buffer, buffer_maxlen, ptr::null_mut(), 0, attrs)]
    libssh2_sftp_close => swoole_libssh2_sftp_close(ops, wait_on; handle: *mut LIBSSH2_SFTP_HANDLE) -> c_int =
        [libssh2_sftp_close_handle(handle)]
    libssh2_sftp_unlink => swoole_libssh2_sftp_unlink(ops, wait_on; sftp: *mut LIBSSH2_SFTP, filename: Text<'_>) -> c_int =
        [libssh2_sftp_unlink_ex(sftp, filename.get().as_ptr(), length(filename.get()))]
    libssh2_sftp_rename => swoole_libssh2_sftp_rename(ops, wait_on; sftp: *mut LIBSSH2_SFTP, sourcefile: Text<'_>, destfile: Text<'_>) -> c_int =
        [libssh2_sftp_rename_ex(sftp, sourcefile.get().as_ptr(), length(sourcefile.get()), destfile.get().as_ptr(), length(destfile.get()), 1 | 2 | 4)]
    libssh2_sftp_mkdir => swoole_libssh2_sftp_mkdir(ops, wait_on; sftp: *mut LIBSSH2_SFTP, path: Text<'_>, mode: c_long) -> c_int =
        [libssh2_sftp_mkdir_ex(sftp, path.get().as_ptr(), length(path.get()), mode)]
    libssh2_sftp_rmdir => swoole_libssh2_sftp_rmdir(ops, wait_on; sftp: *mut LIBSSH2_SFTP, path: Text<'_>) -> c_int =
        [libssh2_sftp_rmdir_ex(sftp, path.get().as_ptr(), length(path.get()))]
    libssh2_userauth_publickey_fromfile => swoole_libssh2_userauth_publickey_fromfile(ops, wait_on; session: *mut LIBSSH2_SESSION, username: Text<'_>, publickey: *const c_char, privatekey: *const c_char, passphrase: *const c_char) -> c_int =
        [libssh2_userauth_publickey_fromfile_ex(session, username.get().as_ptr(), length(username.get()), publickey, privatekey, passphrase)]
    libssh2_userauth_keyboard_interactive => swoole_libssh2_userauth_keyboard_interactive(ops, wait_on; session: *mut LIBSSH2_SESSION, username: Text<'_>, response_callback: Option<KbdintResponse>) -> c_int =
        [libssh2_userauth_keyboard_interactive_ex(session, username.get().as_ptr(), length(username.get()), response_callback)]
}
