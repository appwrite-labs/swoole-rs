/* Generated from crates/ssh2 by tools/headers.sh; do not edit. */
#undef libssh2_session_handshake
#define libssh2_session_handshake(session, sock) swoole_libssh2_session_handshake(ssh2_bound(session), session, session, sock)
#undef libssh2_channel_setenv_ex
#define libssh2_channel_setenv_ex(channel, varname, varname_len, value, value_len) swoole_libssh2_channel_setenv_ex(ssh2_bound(session), session, channel, varname, varname_len, value, value_len)
#undef libssh2_channel_request_pty_ex
#define libssh2_channel_request_pty_ex(channel, term, term_len, modes, modes_len, width, height, width_px, height_px) swoole_libssh2_channel_request_pty_ex(ssh2_bound(session), session, channel, term, term_len, modes, modes_len, width, height, width_px, height_px)
#undef libssh2_channel_flush_ex
#define libssh2_channel_flush_ex(channel, streamid) swoole_libssh2_channel_flush_ex(ssh2_bound(session), session, channel, streamid)
#undef libssh2_channel_read_ex
#define libssh2_channel_read_ex(channel, stream_id, buf, buflen) swoole_libssh2_channel_read_ex(ssh2_bound(session), session, channel, stream_id, buf, buflen)
#undef libssh2_channel_write_ex
#define libssh2_channel_write_ex(channel, stream_id, buf, buflen) swoole_libssh2_channel_write_ex(ssh2_bound(session), session, channel, stream_id, buf, buflen)
#undef libssh2_channel_eof
#define libssh2_channel_eof(channel) swoole_libssh2_channel_eof(ssh2_bound(session), session, channel)
#undef libssh2_channel_close
#define libssh2_channel_close(channel) swoole_libssh2_channel_close(ssh2_bound(session), session, channel)
#undef libssh2_channel_send_eof
#define libssh2_channel_send_eof(channel) swoole_libssh2_channel_send_eof(ssh2_bound(session), session, channel)
#undef libssh2_channel_get_exit_status
#define libssh2_channel_get_exit_status(channel) swoole_libssh2_channel_get_exit_status(ssh2_bound(session), session, channel)
#undef libssh2_channel_request_pty_size_ex
#define libssh2_channel_request_pty_size_ex(channel, width, height, width_px, height_px) swoole_libssh2_channel_request_pty_size_ex(ssh2_bound(session), session, channel, width, height, width_px, height_px)
#undef libssh2_channel_forward_listen_ex
#define libssh2_channel_forward_listen_ex(session, host, port, bound_port, queue_maxsize) swoole_libssh2_channel_forward_listen_ex(ssh2_bound(session), session, session, host, port, bound_port, queue_maxsize)
#undef libssh2_channel_forward_accept
#define libssh2_channel_forward_accept(listener) swoole_libssh2_channel_forward_accept(ssh2_bound(session), session, listener)
#undef libssh2_channel_forward_cancel
#define libssh2_channel_forward_cancel(listener) swoole_libssh2_channel_forward_cancel(ssh2_bound(session), session, listener)
#undef libssh2_sftp_stat_ex
#define libssh2_sftp_stat_ex(sftp, path, path_len, stat_type, attrs) swoole_libssh2_sftp_stat_ex(ssh2_bound(session), session, sftp, path, path_len, stat_type, attrs)
#undef libssh2_sftp_symlink_ex
#define libssh2_sftp_symlink_ex(sftp, path, path_len, target, target_len, link_type) swoole_libssh2_sftp_symlink_ex(ssh2_bound(session), session, sftp, path, path_len, target, target_len, link_type)
#undef libssh2_sftp_tell
#define libssh2_sftp_tell(handle) swoole_libssh2_sftp_tell(ssh2_bound(session), session, handle)
#undef libssh2_sftp_read
#define libssh2_sftp_read(handle, buffer, buffer_maxlen) swoole_libssh2_sftp_read(ssh2_bound(session), session, handle, buffer, buffer_maxlen)
#undef libssh2_sftp_write
#define libssh2_sftp_write(handle, buffer, count) swoole_libssh2_sftp_write(ssh2_bound(session), session, handle, buffer, count)
#undef libssh2_sftp_init
#define libssh2_sftp_init(session) swoole_libssh2_sftp_init(ssh2_bound(session), session, session)
#undef libssh2_scp_recv
#define libssh2_scp_recv(session, path, sb) swoole_libssh2_scp_recv(ssh2_bound(session), session, session, path, sb)
#undef libssh2_scp_send_ex
#define libssh2_scp_send_ex(session, path, mode, size, mtime, atime) swoole_libssh2_scp_send_ex(ssh2_bound(session), session, session, path, mode, size, mtime, atime)
#undef libssh2_sftp_unlink_ex
#define libssh2_sftp_unlink_ex(sftp, filename, filename_len) swoole_libssh2_sftp_unlink_ex(ssh2_bound(session), session, sftp, filename, filename_len)
#undef libssh2_sftp_rename_ex
#define libssh2_sftp_rename_ex(sftp, source_filename, source_filename_len, dest_filename, dest_filename_len, flags) swoole_libssh2_sftp_rename_ex(ssh2_bound(session), session, sftp, source_filename, source_filename_len, dest_filename, dest_filename_len, flags)
#undef libssh2_sftp_mkdir_ex
#define libssh2_sftp_mkdir_ex(sftp, path, path_len, mode) swoole_libssh2_sftp_mkdir_ex(ssh2_bound(session), session, sftp, path, path_len, mode)
#undef libssh2_sftp_rmdir_ex
#define libssh2_sftp_rmdir_ex(sftp, path, path_len) swoole_libssh2_sftp_rmdir_ex(ssh2_bound(session), session, sftp, path, path_len)
#undef libssh2_agent_connect
#define libssh2_agent_connect(agent) swoole_libssh2_agent_connect(ssh2_bound(session), session, agent)
#undef libssh2_agent_list_identities
#define libssh2_agent_list_identities(agent) swoole_libssh2_agent_list_identities(ssh2_bound(session), session, agent)
#undef libssh2_agent_get_identity
#define libssh2_agent_get_identity(agent, store, prev) swoole_libssh2_agent_get_identity(ssh2_bound(session), session, agent, store, prev)
#undef libssh2_agent_userauth
#define libssh2_agent_userauth(agent, username, identity) swoole_libssh2_agent_userauth(ssh2_bound(session), session, agent, username, identity)
#undef libssh2_agent_disconnect
#define libssh2_agent_disconnect(agent) swoole_libssh2_agent_disconnect(ssh2_bound(session), session, agent)
#undef libssh2_userauth_list
#define libssh2_userauth_list(session, username, username_len) swoole_libssh2_userauth_list(ssh2_bound(session), session, session, username, username_len)
#undef libssh2_userauth_password_ex
#define libssh2_userauth_password_ex(session, username, username_len, password, password_len, passwd_change_cb) swoole_libssh2_userauth_password_ex(ssh2_bound(session), session, session, username, username_len, password, password_len, passwd_change_cb)
#undef libssh2_userauth_publickey_fromfile_ex
#define libssh2_userauth_publickey_fromfile_ex(session, username, username_len, publickey, privatekey, passphrase) swoole_libssh2_userauth_publickey_fromfile_ex(ssh2_bound(session), session, session, username, username_len, publickey, privatekey, passphrase)
#undef libssh2_userauth_publickey_frommemory
#define libssh2_userauth_publickey_frommemory(session, username, username_len, publickeyfiledata, publickeyfiledata_len, privatekeyfiledata, privatekeyfiledata_len, passphrase) swoole_libssh2_userauth_publickey_frommemory(ssh2_bound(session), session, session, username, username_len, publickeyfiledata, publickeyfiledata_len, privatekeyfiledata, privatekeyfiledata_len, passphrase)
#undef libssh2_userauth_hostbased_fromfile_ex
#define libssh2_userauth_hostbased_fromfile_ex(session, username, username_len, publickey, privatekey, passphrase, hostname, hostname_len, local_username, local_username_len) swoole_libssh2_userauth_hostbased_fromfile_ex(ssh2_bound(session), session, session, username, username_len, publickey, privatekey, passphrase, hostname, hostname_len, local_username, local_username_len)
#undef libssh2_userauth_authenticated
#define libssh2_userauth_authenticated(session) swoole_libssh2_userauth_authenticated(ssh2_bound(session), session, session)
#undef libssh2_session_disconnect
#define libssh2_session_disconnect(session, description) swoole_libssh2_session_disconnect(ssh2_bound(session), session, session, description)
#undef libssh2_channel_open_session
#define libssh2_channel_open_session(session) swoole_libssh2_channel_open_session(ssh2_bound(session), session, session)
#undef libssh2_channel_shell
#define libssh2_channel_shell(channel) swoole_libssh2_channel_shell(ssh2_bound(session), session, channel)
#undef libssh2_channel_exec
#define libssh2_channel_exec(channel, command) swoole_libssh2_channel_exec(ssh2_bound(session), session, channel, command)
#undef libssh2_channel_read
#define libssh2_channel_read(channel, buf, buflen) swoole_libssh2_channel_read(ssh2_bound(session), session, channel, buf, buflen)
#undef libssh2_channel_write
#define libssh2_channel_write(channel, buf, buflen) swoole_libssh2_channel_write(ssh2_bound(session), session, channel, buf, buflen)
#undef libssh2_channel_direct_tcpip
#define libssh2_channel_direct_tcpip(session, host, port) swoole_libssh2_channel_direct_tcpip(ssh2_bound(session), session, session, host, port)
#undef libssh2_sftp_fstat
#define libssh2_sftp_fstat(handle, attrs) swoole_libssh2_sftp_fstat(ssh2_bound(session), session, handle, attrs)
#undef libssh2_sftp_open
#define libssh2_sftp_open(sftp, filename, flags, mode) swoole_libssh2_sftp_open(ssh2_bound(session), session, sftp, filename, flags, mode)
#undef libssh2_sftp_opendir
#define libssh2_sftp_opendir(sftp, path) swoole_libssh2_sftp_opendir(ssh2_bound(session), session, sftp, path)
#undef libssh2_sftp_readdir
#define libssh2_sftp_readdir(handle, buffer, buffer_maxlen, attrs) swoole_libssh2_sftp_readdir(ssh2_bound(session), session, handle, buffer, buffer_maxlen, attrs)
#undef libssh2_sftp_close
#define libssh2_sftp_close(handle) swoole_libssh2_sftp_close(ssh2_bound(session), session, handle)
#undef libssh2_sftp_unlink
#define libssh2_sftp_unlink(sftp, filename) swoole_libssh2_sftp_unlink(ssh2_bound(session), session, sftp, filename)
#undef libssh2_sftp_rename
#define libssh2_sftp_rename(sftp, sourcefile, destfile) swoole_libssh2_sftp_rename(ssh2_bound(session), session, sftp, sourcefile, destfile)
#undef libssh2_sftp_mkdir
#define libssh2_sftp_mkdir(sftp, path, mode) swoole_libssh2_sftp_mkdir(ssh2_bound(session), session, sftp, path, mode)
#undef libssh2_sftp_rmdir
#define libssh2_sftp_rmdir(sftp, path) swoole_libssh2_sftp_rmdir(ssh2_bound(session), session, sftp, path)
#undef libssh2_userauth_publickey_fromfile
#define libssh2_userauth_publickey_fromfile(session, username, publickey, privatekey, passphrase) swoole_libssh2_userauth_publickey_fromfile(ssh2_bound(session), session, session, username, publickey, privatekey, passphrase)
#undef libssh2_userauth_keyboard_interactive
#define libssh2_userauth_keyboard_interactive(session, username, response_callback) swoole_libssh2_userauth_keyboard_interactive(ssh2_bound(session), session, session, username, response_callback)

