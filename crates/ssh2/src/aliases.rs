//! libssh2.h's convenience macros (`libssh2_sftp_open` for `libssh2_sftp_open_ex`
//! and so on), hooked under their own names: each row is one more export of
//! [`hooks!`](crate), which calls the `_ex` function with the defaults the
//! macro supplies. `[length = text]` measures a C string first, as the macro's
//! `strlen` does.
use super::{
    KbdintResponse, LIBSSH2_CHANNEL, LIBSSH2_SESSION, LIBSSH2_SFTP, LIBSSH2_SFTP_ATTRIBUTES,
    LIBSSH2_SFTP_HANDLE, Narrow, Ops, Wait, c_char, c_int, c_long, c_uint, c_ulong,
    libssh2_channel_direct_tcpip_ex, libssh2_channel_open_ex, libssh2_channel_process_startup,
    libssh2_channel_read_ex, libssh2_channel_write_ex, libssh2_session_disconnect_ex,
    libssh2_sftp_close_handle, libssh2_sftp_fstat_ex, libssh2_sftp_mkdir_ex, libssh2_sftp_open_ex,
    libssh2_sftp_readdir_ex, libssh2_sftp_rename_ex, libssh2_sftp_rmdir_ex, libssh2_sftp_unlink_ex,
    libssh2_userauth_keyboard_interactive_ex, libssh2_userauth_publickey_fromfile_ex, ptr,
};

macro_rules! aliases {
    ($($name:ident => $kind:ident $export:ident($($arg:ident: $ty:ty),*) -> $ret:ty
        => [$($len:ident = $text:ident),*] $function:ident($($call:expr),*);)*) => {
        /// Native call-site forwarding; all argument policy executes in Rust.
        pub const DISPATCH: &str = concat!($(concat!("#undef ", stringify!($name), "\n#define ", stringify!($name),
            "(", hooks!(@params $($arg),*), ") ", stringify!($export), "(ssh2_bound(session), session", $(", ", stringify!($arg),)* ")\n")),*);
        $(hooks!(@$kind [$($len = $text),*] $export($($arg: $ty),*) -> $ret => $function($($call),*));)*
    };
}

aliases! {
    libssh2_session_disconnect => int swoole_libssh2_session_disconnect(session: *mut LIBSSH2_SESSION, description: *const c_char) -> c_int
        => [] libssh2_session_disconnect_ex(session, 11, description, c"".as_ptr());
    libssh2_channel_open_session => pointer swoole_libssh2_channel_open_session(session: *mut LIBSSH2_SESSION) -> *mut LIBSSH2_CHANNEL
        => [] libssh2_channel_open_ex(session, c"session".as_ptr(), 7, 2 * 1024 * 1024, 32768, ptr::null(), 0);
    libssh2_channel_shell => int swoole_libssh2_channel_shell(channel: *mut LIBSSH2_CHANNEL) -> c_int
        => [] libssh2_channel_process_startup(channel, c"shell".as_ptr(), 5, ptr::null(), 0);
    libssh2_channel_exec => int swoole_libssh2_channel_exec(channel: *mut LIBSSH2_CHANNEL, command: *const c_char) -> c_int
        => [command_len = command] libssh2_channel_process_startup(channel, c"exec".as_ptr(), 4, command, command_len);
    libssh2_channel_read => int swoole_libssh2_channel_read(channel: *mut LIBSSH2_CHANNEL, buf: *mut c_char, buflen: usize) -> c_int
        => [] libssh2_channel_read_ex(channel, 0, buf, buflen);
    libssh2_channel_write => int swoole_libssh2_channel_write(channel: *mut LIBSSH2_CHANNEL, buf: *const c_char, buflen: usize) -> c_int
        => [] libssh2_channel_write_ex(channel, 0, buf, buflen);
    libssh2_channel_direct_tcpip => pointer swoole_libssh2_channel_direct_tcpip(session: *mut LIBSSH2_SESSION, host: *const c_char, port: c_int) -> *mut LIBSSH2_CHANNEL
        => [] libssh2_channel_direct_tcpip_ex(session, host, port, c"127.0.0.1".as_ptr(), 22);
    libssh2_sftp_fstat => int swoole_libssh2_sftp_fstat(handle: *mut LIBSSH2_SFTP_HANDLE, attrs: *mut LIBSSH2_SFTP_ATTRIBUTES) -> c_int
        => [] libssh2_sftp_fstat_ex(handle, attrs, 0);
    libssh2_sftp_open => pointer swoole_libssh2_sftp_open(sftp: *mut LIBSSH2_SFTP, filename: *const c_char, flags: c_ulong, mode: c_long) -> *mut LIBSSH2_SFTP_HANDLE
        => [filename_len = filename] libssh2_sftp_open_ex(sftp, filename, filename_len, flags, mode, 0);
    libssh2_sftp_opendir => pointer swoole_libssh2_sftp_opendir(sftp: *mut LIBSSH2_SFTP, path: *const c_char) -> *mut LIBSSH2_SFTP_HANDLE
        => [path_len = path] libssh2_sftp_open_ex(sftp, path, path_len, 0, 0, 1);
    libssh2_sftp_readdir => int swoole_libssh2_sftp_readdir(handle: *mut LIBSSH2_SFTP_HANDLE, buffer: *mut c_char, buffer_maxlen: usize, attrs: *mut LIBSSH2_SFTP_ATTRIBUTES) -> c_int
        => [] libssh2_sftp_readdir_ex(handle, buffer, buffer_maxlen, ptr::null_mut(), 0, attrs);
    libssh2_sftp_close => int swoole_libssh2_sftp_close(handle: *mut LIBSSH2_SFTP_HANDLE) -> c_int
        => [] libssh2_sftp_close_handle(handle);
    libssh2_sftp_unlink => int swoole_libssh2_sftp_unlink(sftp: *mut LIBSSH2_SFTP, filename: *const c_char) -> c_int
        => [filename_len = filename] libssh2_sftp_unlink_ex(sftp, filename, filename_len);
    libssh2_sftp_rename => int swoole_libssh2_sftp_rename(sftp: *mut LIBSSH2_SFTP, sourcefile: *const c_char, destfile: *const c_char) -> c_int
        => [sourcefile_len = sourcefile, destfile_len = destfile] libssh2_sftp_rename_ex(sftp, sourcefile, sourcefile_len, destfile, destfile_len, 1 | 2 | 4);
    libssh2_sftp_mkdir => int swoole_libssh2_sftp_mkdir(sftp: *mut LIBSSH2_SFTP, path: *const c_char, mode: c_long) -> c_int
        => [path_len = path] libssh2_sftp_mkdir_ex(sftp, path, path_len, mode);
    libssh2_sftp_rmdir => int swoole_libssh2_sftp_rmdir(sftp: *mut LIBSSH2_SFTP, path: *const c_char) -> c_int
        => [path_len = path] libssh2_sftp_rmdir_ex(sftp, path, path_len);
    libssh2_userauth_publickey_fromfile => int swoole_libssh2_userauth_publickey_fromfile(session: *mut LIBSSH2_SESSION, username: *const c_char, publickey: *const c_char, privatekey: *const c_char, passphrase: *const c_char) -> c_int
        => [username_len = username] libssh2_userauth_publickey_fromfile_ex(session, username, username_len, publickey, privatekey, passphrase);
    libssh2_userauth_keyboard_interactive => int swoole_libssh2_userauth_keyboard_interactive(session: *mut LIBSSH2_SESSION, username: *const c_char, response_callback: Option<KbdintResponse>) -> c_int
        => [username_len = username] libssh2_userauth_keyboard_interactive_ex(session, username, username_len, response_callback);
}
