//! Convenience/default argument policy for the original optional SDK aliases.
use super::{
    CStr, Channel, KbdintResponse, LIBSSH2_CHANNEL, LIBSSH2_SESSION, LIBSSH2_SFTP,
    LIBSSH2_SFTP_ATTRIBUTES, LIBSSH2_SFTP_HANDLE, Narrow, Ops, Session, Sftp, SftpHandle, Text,
    Wait, c_char, c_int, c_long, c_ulong, direct_tcpip, disconnect, fstat, length, ptr,
    swoole_libssh2_sftp_mkdir_ex, swoole_libssh2_sftp_rename_ex, swoole_libssh2_sftp_rmdir_ex,
    swoole_libssh2_sftp_unlink_ex, swoole_libssh2_userauth_publickey_fromfile_ex,
};
use swoole_rs_ssh2_sys::{
    libssh2_channel_open_ex, libssh2_channel_process_startup, libssh2_channel_read_ex,
    libssh2_channel_write_ex, libssh2_sftp_close_handle, libssh2_sftp_open_ex,
    libssh2_sftp_readdir_ex, libssh2_userauth_keyboard_interactive_ex,
};

macro_rules! aliases {
    ($($name:ident => $export:ident($ops:ident, $wait:ident; $($arg:ident: $ty:ty),*) -> $ret:ty $body:block)*) => {
        /// Native call-site forwarding; all argument policy executes in Rust.
        pub const DISPATCH: &str = concat!($(concat!("#undef ", stringify!($name), "\n#define ", stringify!($name),
            "(", hooks!(@params $($arg),*), ") ", stringify!($export), "(ssh2_bound(session), session", $(", ", stringify!($arg),)* ")\n")),*);
        $(
            #[doc = concat!("Original `", stringify!($name), "` convenience operation.")]
            /// # Safety
            /// The caller lends the original SDK arguments, the live session to wait on and
            /// the host table.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn $export($ops: &Ops, $wait: Session, $($arg: $ty),*) -> $ret $body
        )*
    };
}

/// `libssh2_channel_process_startup(channel, request, message)`, retried; no
/// message is a null one.
fn process_startup(
    wait: &Wait<'_>,
    channel: Channel,
    request: &CStr,
    message: Option<&CStr>,
) -> c_int {
    let (message, message_len) = message.map_or((ptr::null(), 0), |m| (m.as_ptr(), length(m)));
    // SAFETY: the live channel, and the strings libssh2 reads for the lengths given.
    wait.call(|| unsafe {
        libssh2_channel_process_startup(
            channel.0,
            request.as_ptr(),
            length(request),
            message,
            message_len,
        )
    })
}

/// `libssh2_sftp_open_ex(sftp, path, flags, mode, open_type)`, retried.
fn sftp_open(
    wait: &Wait<'_>,
    sftp: Sftp,
    path: &CStr,
    flags: c_ulong,
    mode: c_long,
    open_type: c_int,
) -> *mut LIBSSH2_SFTP_HANDLE {
    // SAFETY: the live SFTP session, and the path libssh2 reads for the length given.
    wait.call_pointer(|| unsafe {
        libssh2_sftp_open_ex(sftp.0, path.as_ptr(), length(path), flags, mode, open_type)
    })
}

aliases! {
    libssh2_session_disconnect => swoole_libssh2_session_disconnect(ops, wait_on; session: Session, description: Text<'_>) -> c_int {
        Wait { ops, session: wait_on }.call(|| disconnect(session, 11, description, c"".into()))
    }
    libssh2_channel_open_session => swoole_libssh2_channel_open_session(ops, wait_on; session: *mut LIBSSH2_SESSION) -> *mut LIBSSH2_CHANNEL {
        // SAFETY: the caller's live session.
        Wait { ops, session: wait_on }.call_pointer(|| unsafe { libssh2_channel_open_ex(session, c"session".as_ptr(), 7, 2 * 1024 * 1024, 32768, ptr::null(), 0) })
    }
    libssh2_channel_shell => swoole_libssh2_channel_shell(ops, wait_on; channel: Channel) -> c_int {
        process_startup(&Wait { ops, session: wait_on }, channel, c"shell", None)
    }
    libssh2_channel_exec => swoole_libssh2_channel_exec(ops, wait_on; channel: Channel, command: Text<'_>) -> c_int {
        process_startup(&Wait { ops, session: wait_on }, channel, c"exec", Some(command.get()))
    }
    libssh2_channel_read => swoole_libssh2_channel_read(ops, wait_on; channel: *mut LIBSSH2_CHANNEL, buf: *mut c_char, buflen: usize) -> c_int {
        // SAFETY: the caller's live channel and `buflen` writable bytes at `buf`.
        Wait { ops, session: wait_on }.call(|| unsafe { libssh2_channel_read_ex(channel, 0, buf, buflen) }.narrow())
    }
    libssh2_channel_write => swoole_libssh2_channel_write(ops, wait_on; channel: *mut LIBSSH2_CHANNEL, buf: *const c_char, buflen: usize) -> c_int {
        // SAFETY: the caller's live channel and `buflen` readable bytes at `buf`.
        Wait { ops, session: wait_on }.call(|| unsafe { libssh2_channel_write_ex(channel, 0, buf, buflen) }.narrow())
    }
    libssh2_channel_direct_tcpip => swoole_libssh2_channel_direct_tcpip(ops, wait_on; session: Session, host: Text<'_>, port: c_int) -> *mut LIBSSH2_CHANNEL {
        Wait { ops, session: wait_on }.call_pointer(|| direct_tcpip(session, host, port, c"127.0.0.1".into(), 22))
    }
    libssh2_sftp_fstat => swoole_libssh2_sftp_fstat(ops, wait_on; handle: SftpHandle, attrs: &mut LIBSSH2_SFTP_ATTRIBUTES) -> c_int {
        Wait { ops, session: wait_on }.call(|| fstat(handle, attrs, 0))
    }
    libssh2_sftp_open => swoole_libssh2_sftp_open(ops, wait_on; sftp: Sftp, filename: Text<'_>, flags: c_ulong, mode: c_long) -> *mut LIBSSH2_SFTP_HANDLE {
        sftp_open(&Wait { ops, session: wait_on }, sftp, filename.get(), flags, mode, 0)
    }
    libssh2_sftp_opendir => swoole_libssh2_sftp_opendir(ops, wait_on; sftp: Sftp, path: Text<'_>) -> *mut LIBSSH2_SFTP_HANDLE {
        sftp_open(&Wait { ops, session: wait_on }, sftp, path.get(), 0, 0, 1)
    }
    libssh2_sftp_readdir => swoole_libssh2_sftp_readdir(ops, wait_on; handle: *mut LIBSSH2_SFTP_HANDLE, buffer: *mut c_char, buffer_maxlen: usize, attrs: *mut LIBSSH2_SFTP_ATTRIBUTES) -> c_int {
        // SAFETY: the caller's live handle, `buffer_maxlen` writable bytes at `buffer`
        // and its attributes record.
        Wait { ops, session: wait_on }.call(|| unsafe { libssh2_sftp_readdir_ex(handle, buffer, buffer_maxlen, ptr::null_mut(), 0, attrs) })
    }
    libssh2_sftp_close => swoole_libssh2_sftp_close(ops, wait_on; handle: *mut LIBSSH2_SFTP_HANDLE) -> c_int {
        // SAFETY: the caller's live handle, which libssh2 frees once it closes.
        Wait { ops, session: wait_on }.call(|| unsafe { libssh2_sftp_close_handle(handle) })
    }
    libssh2_sftp_unlink => swoole_libssh2_sftp_unlink(ops, wait_on; sftp: *mut LIBSSH2_SFTP, filename: Text<'_>) -> c_int {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_sftp_unlink_ex(ops, wait_on, sftp, filename.get().as_ptr(), length(filename.get())) }
    }
    libssh2_sftp_rename => swoole_libssh2_sftp_rename(ops, wait_on; sftp: *mut LIBSSH2_SFTP, sourcefile: Text<'_>, destfile: Text<'_>) -> c_int {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_sftp_rename_ex(ops, wait_on, sftp, sourcefile.get().as_ptr(), length(sourcefile.get()), destfile.get().as_ptr(), length(destfile.get()), 1 | 2 | 4) }
    }
    libssh2_sftp_mkdir => swoole_libssh2_sftp_mkdir(ops, wait_on; sftp: *mut LIBSSH2_SFTP, path: Text<'_>, mode: c_long) -> c_int {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_sftp_mkdir_ex(ops, wait_on, sftp, path.get().as_ptr(), length(path.get()), mode) }
    }
    libssh2_sftp_rmdir => swoole_libssh2_sftp_rmdir(ops, wait_on; sftp: *mut LIBSSH2_SFTP, path: Text<'_>) -> c_int {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_sftp_rmdir_ex(ops, wait_on, sftp, path.get().as_ptr(), length(path.get())) }
    }
    libssh2_userauth_publickey_fromfile => swoole_libssh2_userauth_publickey_fromfile(ops, wait_on; session: *mut LIBSSH2_SESSION, username: Text<'_>, publickey: *const c_char, privatekey: *const c_char, passphrase: *const c_char) -> c_int {
        // SAFETY: the original alias arguments satisfy the canonical hook contract.
        unsafe { swoole_libssh2_userauth_publickey_fromfile_ex(ops, wait_on, session, username.get().as_ptr(), length(username.get()), publickey, privatekey, passphrase) }
    }
    libssh2_userauth_keyboard_interactive => swoole_libssh2_userauth_keyboard_interactive(ops, wait_on; session: *mut LIBSSH2_SESSION, username: Text<'_>, response_callback: Option<KbdintResponse>) -> c_int {
        let username = username.get();
        // SAFETY: the caller's live session and callback, and the name libssh2 reads
        // for the length given.
        Wait { ops, session: wait_on }.call(|| unsafe { libssh2_userauth_keyboard_interactive_ex(session, username.as_ptr(), length(username), response_callback) })
    }
}
