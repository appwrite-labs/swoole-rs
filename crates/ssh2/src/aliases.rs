//! Convenience/default argument policy for the original optional SDK aliases.
use super::{
    KbdintResponse, LIBSSH2_CHANNEL, LIBSSH2_SESSION, LIBSSH2_SFTP, LIBSSH2_SFTP_ATTRIBUTES,
    LIBSSH2_SFTP_HANDLE, Ops, c_char, c_int, c_long, c_uint, c_ulong, ptr,
    swoole_libssh2_channel_direct_tcpip_ex, swoole_libssh2_channel_open_ex,
    swoole_libssh2_channel_process_startup, swoole_libssh2_channel_read_ex,
    swoole_libssh2_channel_write_ex, swoole_libssh2_session_disconnect_ex,
    swoole_libssh2_sftp_close_handle, swoole_libssh2_sftp_fstat_ex, swoole_libssh2_sftp_mkdir_ex,
    swoole_libssh2_sftp_open_ex, swoole_libssh2_sftp_readdir_ex, swoole_libssh2_sftp_rename_ex,
    swoole_libssh2_sftp_rmdir_ex, swoole_libssh2_sftp_unlink_ex,
    swoole_libssh2_userauth_keyboard_interactive_ex, swoole_libssh2_userauth_publickey_fromfile_ex,
};

macro_rules! aliases {
    ($($name:ident => $export:ident($ops:ident, $wait:ident; $($arg:ident: $ty:ty),*) -> $ret:ty $body:block)*) => {
        /// Native call-site forwarding; all argument policy executes in Rust.
        pub const DISPATCH: &str = concat!($(concat!("#undef ", stringify!($name), "\n#define ", stringify!($name),
            "(", hooks!(@params $($arg),*), ") ", stringify!($export), "(ssh2_bound(session), session", $(", ", stringify!($arg),)* ")\n")),*);
        $(
            #[doc = concat!("Original `", stringify!($name), "` convenience operation.")]
            /// # Safety
            /// The caller lends the original SDK arguments, live wait session and host table.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $export($ops: *const Ops, $wait: *mut LIBSSH2_SESSION, $($arg: $ty),*) -> $ret $body
        )*
    };
}

aliases! {
    libssh2_session_disconnect => swoole_libssh2_session_disconnect(ops, wait_on; session: *mut LIBSSH2_SESSION, description: *const c_char) -> c_int {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_session_disconnect_ex(ops, wait_on, session, 11, description, c"".as_ptr()) }
    }
    libssh2_channel_open_session => swoole_libssh2_channel_open_session(ops, wait_on; session: *mut LIBSSH2_SESSION) -> *mut LIBSSH2_CHANNEL {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_channel_open_ex(ops, wait_on, session, c"session".as_ptr(), 7, 2 * 1024 * 1024, 32768, ptr::null(), 0) }
    }
    libssh2_channel_shell => swoole_libssh2_channel_shell(ops, wait_on; channel: *mut LIBSSH2_CHANNEL) -> c_int {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_channel_process_startup(ops, wait_on, channel, c"shell".as_ptr(), 5, ptr::null(), 0) }
    }
    libssh2_channel_exec => swoole_libssh2_channel_exec(ops, wait_on; channel: *mut LIBSSH2_CHANNEL, command: *const c_char) -> c_int {
        // SAFETY: the caller lends the original NUL-terminated SDK argument.
        let command_len = unsafe { libc::strlen(command) };
        #[expect(clippy::cast_possible_truncation, reason = "the upstream alias casts strlen to unsigned int")]
        let command_len = command_len as c_uint;
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_channel_process_startup(ops, wait_on, channel, c"exec".as_ptr(), 4, command, command_len) }
    }
    libssh2_channel_read => swoole_libssh2_channel_read(ops, wait_on; channel: *mut LIBSSH2_CHANNEL, buf: *mut c_char, buflen: usize) -> c_int {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_channel_read_ex(ops, wait_on, channel, 0, buf, buflen) }
    }
    libssh2_channel_write => swoole_libssh2_channel_write(ops, wait_on; channel: *mut LIBSSH2_CHANNEL, buf: *const c_char, buflen: usize) -> c_int {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_channel_write_ex(ops, wait_on, channel, 0, buf, buflen) }
    }
    libssh2_channel_direct_tcpip => swoole_libssh2_channel_direct_tcpip(ops, wait_on; session: *mut LIBSSH2_SESSION, host: *const c_char, port: c_int) -> *mut LIBSSH2_CHANNEL {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_channel_direct_tcpip_ex(ops, wait_on, session, host, port, c"127.0.0.1".as_ptr(), 22) }
    }
    libssh2_sftp_fstat => swoole_libssh2_sftp_fstat(ops, wait_on; handle: *mut LIBSSH2_SFTP_HANDLE, attrs: *mut LIBSSH2_SFTP_ATTRIBUTES) -> c_int {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_sftp_fstat_ex(ops, wait_on, handle, attrs, 0) }
    }
    libssh2_sftp_open => swoole_libssh2_sftp_open(ops, wait_on; sftp: *mut LIBSSH2_SFTP, filename: *const c_char, flags: c_ulong, mode: c_long) -> *mut LIBSSH2_SFTP_HANDLE {
        // SAFETY: the caller lends the original NUL-terminated SDK argument.
        let filename_len = unsafe { libc::strlen(filename) };
        #[expect(clippy::cast_possible_truncation, reason = "the upstream alias casts strlen to unsigned int")]
        let filename_len = filename_len as c_uint;
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_sftp_open_ex(ops, wait_on, sftp, filename, filename_len, flags, mode, 0) }
    }
    libssh2_sftp_opendir => swoole_libssh2_sftp_opendir(ops, wait_on; sftp: *mut LIBSSH2_SFTP, path: *const c_char) -> *mut LIBSSH2_SFTP_HANDLE {
        // SAFETY: the caller lends the original NUL-terminated SDK argument.
        let path_len = unsafe { libc::strlen(path) };
        #[expect(clippy::cast_possible_truncation, reason = "the upstream alias casts strlen to unsigned int")]
        let path_len = path_len as c_uint;
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_sftp_open_ex(ops, wait_on, sftp, path, path_len, 0, 0, 1) }
    }
    libssh2_sftp_readdir => swoole_libssh2_sftp_readdir(ops, wait_on; handle: *mut LIBSSH2_SFTP_HANDLE, buffer: *mut c_char, buffer_maxlen: usize, attrs: *mut LIBSSH2_SFTP_ATTRIBUTES) -> c_int {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_sftp_readdir_ex(ops, wait_on, handle, buffer, buffer_maxlen, ptr::null_mut(), 0, attrs) }
    }
    libssh2_sftp_close => swoole_libssh2_sftp_close(ops, wait_on; handle: *mut LIBSSH2_SFTP_HANDLE) -> c_int {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_sftp_close_handle(ops, wait_on, handle) }
    }
    libssh2_sftp_unlink => swoole_libssh2_sftp_unlink(ops, wait_on; sftp: *mut LIBSSH2_SFTP, filename: *const c_char) -> c_int {
        // SAFETY: the caller lends the original NUL-terminated SDK argument.
        let filename_len = unsafe { libc::strlen(filename) };
        #[expect(clippy::cast_possible_truncation, reason = "the upstream alias casts strlen to unsigned int")]
        let filename_len = filename_len as c_uint;
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_sftp_unlink_ex(ops, wait_on, sftp, filename, filename_len) }
    }
    libssh2_sftp_rename => swoole_libssh2_sftp_rename(ops, wait_on; sftp: *mut LIBSSH2_SFTP, sourcefile: *const c_char, destfile: *const c_char) -> c_int {
        // SAFETY: the caller lends the original NUL-terminated SDK argument.
        let sourcefile_len = unsafe { libc::strlen(sourcefile) };
        #[expect(clippy::cast_possible_truncation, reason = "the upstream alias casts strlen to unsigned int")]
        let sourcefile_len = sourcefile_len as c_uint;
        // SAFETY: the caller lends the original NUL-terminated SDK argument.
        let destfile_len = unsafe { libc::strlen(destfile) };
        #[expect(clippy::cast_possible_truncation, reason = "the upstream alias casts strlen to unsigned int")]
        let destfile_len = destfile_len as c_uint;
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_sftp_rename_ex(ops, wait_on, sftp, sourcefile, sourcefile_len, destfile, destfile_len, 1 | 2 | 4) }
    }
    libssh2_sftp_mkdir => swoole_libssh2_sftp_mkdir(ops, wait_on; sftp: *mut LIBSSH2_SFTP, path: *const c_char, mode: c_long) -> c_int {
        // SAFETY: the caller lends the original NUL-terminated SDK argument.
        let path_len = unsafe { libc::strlen(path) };
        #[expect(clippy::cast_possible_truncation, reason = "the upstream alias casts strlen to unsigned int")]
        let path_len = path_len as c_uint;
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_sftp_mkdir_ex(ops, wait_on, sftp, path, path_len, mode) }
    }
    libssh2_sftp_rmdir => swoole_libssh2_sftp_rmdir(ops, wait_on; sftp: *mut LIBSSH2_SFTP, path: *const c_char) -> c_int {
        // SAFETY: the caller lends the original NUL-terminated SDK argument.
        let path_len = unsafe { libc::strlen(path) };
        #[expect(clippy::cast_possible_truncation, reason = "the upstream alias casts strlen to unsigned int")]
        let path_len = path_len as c_uint;
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_sftp_rmdir_ex(ops, wait_on, sftp, path, path_len) }
    }
    libssh2_userauth_publickey_fromfile => swoole_libssh2_userauth_publickey_fromfile(ops, wait_on; session: *mut LIBSSH2_SESSION, username: *const c_char, publickey: *const c_char, privatekey: *const c_char, passphrase: *const c_char) -> c_int {
        // SAFETY: the caller lends the original NUL-terminated SDK argument.
        let username_len = unsafe { libc::strlen(username) };
        #[expect(clippy::cast_possible_truncation, reason = "the upstream alias casts strlen to unsigned int")]
        let username_len = username_len as c_uint;
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_userauth_publickey_fromfile_ex(ops, wait_on, session, username, username_len, publickey, privatekey, passphrase) }
    }
    libssh2_userauth_keyboard_interactive => swoole_libssh2_userauth_keyboard_interactive(ops, wait_on; session: *mut LIBSSH2_SESSION, username: *const c_char, response_callback: Option<KbdintResponse>) -> c_int {
        // SAFETY: the caller lends the original NUL-terminated SDK argument.
        let username_len = unsafe { libc::strlen(username) };
        #[expect(clippy::cast_possible_truncation, reason = "the upstream alias casts strlen to unsigned int")]
        let username_len = username_len as c_uint;
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_userauth_keyboard_interactive_ex(ops, wait_on, session, username, username_len, response_callback) }
    }
}
