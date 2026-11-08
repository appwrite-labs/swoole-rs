//! The libssh2 calls Swoole's bundled `ext/ssh2` makes through its coroutine
//! hook (`php_swoole_ssh2_hook.h`), as `libssh2.h` and `libssh2_sftp.h`
//! declare them (libssh2 1.2 and later, which `config.m4` requires). The
//! extension links the system libssh2 itself (`--with-swoole-ssh2`); nothing
//! here links it, so the workspace builds and tests on machines without it.

use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};

macro_rules! opaque {
    ($($name:ident),*) => {$(
        /// Only ever passed by pointer.
        #[repr(C)]
        pub struct $name {
            _opaque: [u8; 0],
        }
    )*};
}

opaque!(
    LIBSSH2_SESSION,
    LIBSSH2_CHANNEL,
    LIBSSH2_LISTENER,
    LIBSSH2_SFTP,
    LIBSSH2_SFTP_HANDLE,
    LIBSSH2_SFTP_ATTRIBUTES,
    LIBSSH2_AGENT,
    libssh2_agent_publickey
);

/// `LIBSSH2_ERROR_SOCKET_NONE` and `LIBSSH2_ERROR_EAGAIN`.
pub const LIBSSH2_ERROR_SOCKET_NONE: c_int = -1;
pub const LIBSSH2_ERROR_EAGAIN: c_int = -37;
/// `LIBSSH2_SESSION_BLOCK_OUTBOUND`.
pub const LIBSSH2_SESSION_BLOCK_OUTBOUND: c_int = 2;

/// `LIBSSH2_PASSWD_CHANGEREQ_FUNC`.
pub type PasswdChangeReq = unsafe extern "C" fn(
    session: *mut LIBSSH2_SESSION,
    newpw: *mut *mut c_char,
    newpw_len: *mut c_int,
    abstract_: *mut *mut c_void,
);
/// `LIBSSH2_USERAUTH_KBDINT_RESPONSE_FUNC`; the prompts and responses are
/// passed through untouched.
pub type KbdintResponse = unsafe extern "C" fn(
    name: *const c_char,
    name_len: c_int,
    instruction: *const c_char,
    instruction_len: c_int,
    num_prompts: c_int,
    prompts: *const c_void,
    responses: *mut c_void,
    abstract_: *mut *mut c_void,
);

unsafe extern "C" {
    pub fn libssh2_session_abstract(session: *mut LIBSSH2_SESSION) -> *mut *mut c_void;
    pub fn libssh2_session_block_directions(session: *mut LIBSSH2_SESSION) -> c_int;
    pub fn libssh2_session_last_errno(session: *mut LIBSSH2_SESSION) -> c_int;
    pub fn libssh2_session_handshake(session: *mut LIBSSH2_SESSION, sock: c_int) -> c_int;
    pub fn libssh2_session_disconnect_ex(
        session: *mut LIBSSH2_SESSION,
        reason: c_int,
        description: *const c_char,
        lang: *const c_char,
    ) -> c_int;
    pub fn libssh2_channel_open_ex(
        session: *mut LIBSSH2_SESSION,
        channel_type: *const c_char,
        channel_type_len: c_uint,
        window_size: c_uint,
        packet_size: c_uint,
        message: *const c_char,
        message_len: c_uint,
    ) -> *mut LIBSSH2_CHANNEL;
    pub fn libssh2_channel_setenv_ex(
        channel: *mut LIBSSH2_CHANNEL,
        varname: *const c_char,
        varname_len: c_uint,
        value: *const c_char,
        value_len: c_uint,
    ) -> c_int;
    pub fn libssh2_channel_request_pty_ex(
        channel: *mut LIBSSH2_CHANNEL,
        term: *const c_char,
        term_len: c_uint,
        modes: *const c_char,
        modes_len: c_uint,
        width: c_int,
        height: c_int,
        width_px: c_int,
        height_px: c_int,
    ) -> c_int;
    pub fn libssh2_channel_process_startup(
        channel: *mut LIBSSH2_CHANNEL,
        request: *const c_char,
        request_len: c_uint,
        message: *const c_char,
        message_len: c_uint,
    ) -> c_int;
    pub fn libssh2_channel_flush_ex(channel: *mut LIBSSH2_CHANNEL, streamid: c_int) -> c_int;
    pub fn libssh2_channel_read_ex(
        channel: *mut LIBSSH2_CHANNEL,
        stream_id: c_int,
        buf: *mut c_char,
        buflen: usize,
    ) -> isize;
    pub fn libssh2_channel_write_ex(
        channel: *mut LIBSSH2_CHANNEL,
        stream_id: c_int,
        buf: *const c_char,
        buflen: usize,
    ) -> isize;
    pub fn libssh2_channel_eof(channel: *mut LIBSSH2_CHANNEL) -> c_int;
    pub fn libssh2_channel_close(channel: *mut LIBSSH2_CHANNEL) -> c_int;
    pub fn libssh2_channel_send_eof(channel: *mut LIBSSH2_CHANNEL) -> c_int;
    pub fn libssh2_channel_get_exit_status(channel: *mut LIBSSH2_CHANNEL) -> c_int;
    pub fn libssh2_channel_request_pty_size_ex(
        channel: *mut LIBSSH2_CHANNEL,
        width: c_int,
        height: c_int,
        width_px: c_int,
        height_px: c_int,
    ) -> c_int;
    pub fn libssh2_channel_forward_listen_ex(
        session: *mut LIBSSH2_SESSION,
        host: *const c_char,
        port: c_int,
        bound_port: *mut c_int,
        queue_maxsize: c_int,
    ) -> *mut LIBSSH2_LISTENER;
    pub fn libssh2_channel_forward_accept(listener: *mut LIBSSH2_LISTENER) -> *mut LIBSSH2_CHANNEL;
    pub fn libssh2_channel_forward_cancel(listener: *mut LIBSSH2_LISTENER) -> c_int;
    pub fn libssh2_channel_direct_tcpip_ex(
        session: *mut LIBSSH2_SESSION,
        host: *const c_char,
        port: c_int,
        shost: *const c_char,
        sport: c_int,
    ) -> *mut LIBSSH2_CHANNEL;
    pub fn libssh2_sftp_fstat_ex(
        handle: *mut LIBSSH2_SFTP_HANDLE,
        attrs: *mut LIBSSH2_SFTP_ATTRIBUTES,
        setstat: c_int,
    ) -> c_int;
    pub fn libssh2_sftp_stat_ex(
        sftp: *mut LIBSSH2_SFTP,
        path: *const c_char,
        path_len: c_uint,
        stat_type: c_int,
        attrs: *mut LIBSSH2_SFTP_ATTRIBUTES,
    ) -> c_int;
    pub fn libssh2_sftp_symlink_ex(
        sftp: *mut LIBSSH2_SFTP,
        path: *const c_char,
        path_len: c_uint,
        target: *mut c_char,
        target_len: c_uint,
        link_type: c_int,
    ) -> c_int;
    pub fn libssh2_sftp_open_ex(
        sftp: *mut LIBSSH2_SFTP,
        filename: *const c_char,
        filename_len: c_uint,
        flags: c_ulong,
        mode: c_long,
        open_type: c_int,
    ) -> *mut LIBSSH2_SFTP_HANDLE;
    pub fn libssh2_sftp_readdir_ex(
        handle: *mut LIBSSH2_SFTP_HANDLE,
        buffer: *mut c_char,
        buffer_maxlen: usize,
        longentry: *mut c_char,
        longentry_maxlen: usize,
        attrs: *mut LIBSSH2_SFTP_ATTRIBUTES,
    ) -> c_int;
    pub fn libssh2_sftp_tell(handle: *mut LIBSSH2_SFTP_HANDLE) -> usize;
    pub fn libssh2_sftp_read(
        handle: *mut LIBSSH2_SFTP_HANDLE,
        buffer: *mut c_char,
        buffer_maxlen: usize,
    ) -> isize;
    pub fn libssh2_sftp_write(
        handle: *mut LIBSSH2_SFTP_HANDLE,
        buffer: *const c_char,
        count: usize,
    ) -> isize;
    pub fn libssh2_sftp_init(session: *mut LIBSSH2_SESSION) -> *mut LIBSSH2_SFTP;
    pub fn libssh2_scp_recv(
        session: *mut LIBSSH2_SESSION,
        path: *const c_char,
        sb: *mut libc::stat,
    ) -> *mut LIBSSH2_CHANNEL;
    pub fn libssh2_scp_send_ex(
        session: *mut LIBSSH2_SESSION,
        path: *const c_char,
        mode: c_int,
        size: usize,
        mtime: c_long,
        atime: c_long,
    ) -> *mut LIBSSH2_CHANNEL;
    pub fn libssh2_sftp_close_handle(handle: *mut LIBSSH2_SFTP_HANDLE) -> c_int;
    pub fn libssh2_sftp_unlink_ex(
        sftp: *mut LIBSSH2_SFTP,
        filename: *const c_char,
        filename_len: c_uint,
    ) -> c_int;
    pub fn libssh2_sftp_rename_ex(
        sftp: *mut LIBSSH2_SFTP,
        source_filename: *const c_char,
        source_filename_len: c_uint,
        dest_filename: *const c_char,
        dest_filename_len: c_uint,
        flags: c_long,
    ) -> c_int;
    pub fn libssh2_sftp_mkdir_ex(
        sftp: *mut LIBSSH2_SFTP,
        path: *const c_char,
        path_len: c_uint,
        mode: c_long,
    ) -> c_int;
    pub fn libssh2_sftp_rmdir_ex(
        sftp: *mut LIBSSH2_SFTP,
        path: *const c_char,
        path_len: c_uint,
    ) -> c_int;
    pub fn libssh2_agent_connect(agent: *mut LIBSSH2_AGENT) -> c_int;
    pub fn libssh2_agent_list_identities(agent: *mut LIBSSH2_AGENT) -> c_int;
    pub fn libssh2_agent_get_identity(
        agent: *mut LIBSSH2_AGENT,
        store: *mut *mut libssh2_agent_publickey,
        prev: *mut libssh2_agent_publickey,
    ) -> c_int;
    pub fn libssh2_agent_userauth(
        agent: *mut LIBSSH2_AGENT,
        username: *const c_char,
        identity: *mut libssh2_agent_publickey,
    ) -> c_int;
    pub fn libssh2_agent_disconnect(agent: *mut LIBSSH2_AGENT) -> c_int;
    pub fn libssh2_userauth_list(
        session: *mut LIBSSH2_SESSION,
        username: *const c_char,
        username_len: c_uint,
    ) -> *mut c_char;
    pub fn libssh2_userauth_password_ex(
        session: *mut LIBSSH2_SESSION,
        username: *const c_char,
        username_len: c_uint,
        password: *const c_char,
        password_len: c_uint,
        passwd_change_cb: Option<PasswdChangeReq>,
    ) -> c_int;
    pub fn libssh2_userauth_publickey_fromfile_ex(
        session: *mut LIBSSH2_SESSION,
        username: *const c_char,
        username_len: c_uint,
        publickey: *const c_char,
        privatekey: *const c_char,
        passphrase: *const c_char,
    ) -> c_int;
    pub fn libssh2_userauth_publickey_frommemory(
        session: *mut LIBSSH2_SESSION,
        username: *const c_char,
        username_len: usize,
        publickeyfiledata: *const c_char,
        publickeyfiledata_len: usize,
        privatekeyfiledata: *const c_char,
        privatekeyfiledata_len: usize,
        passphrase: *const c_char,
    ) -> c_int;
    pub fn libssh2_userauth_hostbased_fromfile_ex(
        session: *mut LIBSSH2_SESSION,
        username: *const c_char,
        username_len: c_uint,
        publickey: *const c_char,
        privatekey: *const c_char,
        passphrase: *const c_char,
        hostname: *const c_char,
        hostname_len: c_uint,
        local_username: *const c_char,
        local_username_len: c_uint,
    ) -> c_int;
    pub fn libssh2_userauth_keyboard_interactive_ex(
        session: *mut LIBSSH2_SESSION,
        username: *const c_char,
        username_len: c_uint,
        response_callback: Option<KbdintResponse>,
    ) -> c_int;
    pub fn libssh2_userauth_authenticated(session: *mut LIBSSH2_SESSION) -> c_int;
}
