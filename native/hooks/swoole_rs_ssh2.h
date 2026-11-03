#ifndef SWOOLE_RS_SSH2_H
#define SWOOLE_RS_SSH2_H
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
/* The coroutine ssh2 hook (Rust, crates/ssh2, --with-swoole-ssh2): each
 * blocking libssh2 call the bundled ext/ssh2 makes, retried while libssh2 says
 * EAGAIN, the coroutine waiting on the session's socket in between.
 * php_swoole_ssh2_hook.h calls swoole_<function>(ops, session, arguments...)
 * once no other coroutine is bound to the socket. Include after libssh2.h and
 * libssh2_sftp.h. */
struct swr_v1_ssh2_ops {
    /* Park the coroutine until event (SW_EVENT_READ/WRITE) on the session's
     * socket: false if the wait failed. */
    bool (*wait)(LIBSSH2_SESSION *session, int event);
};
int swoole_libssh2_session_handshake(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session, libssh2_socket_t sock);
int swoole_libssh2_session_disconnect_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session, int reason, const char *description, const char *lang);
LIBSSH2_CHANNEL *swoole_libssh2_channel_open_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session, const char *channel_type, unsigned int channel_type_len, unsigned int window_size, unsigned int packet_size, const char *message, unsigned int message_len);
int swoole_libssh2_channel_setenv_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL *channel, const char *varname, unsigned int varname_len, const char *value, unsigned int value_len);
int swoole_libssh2_channel_request_pty_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL *channel, const char *term, unsigned int term_len, const char *modes, unsigned int modes_len, int width, int height, int width_px, int height_px);
int swoole_libssh2_channel_process_startup(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL *channel, const char *request, unsigned int request_len, const char *message, unsigned int message_len);
int swoole_libssh2_channel_flush_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL *channel, int streamid);
int swoole_libssh2_channel_read_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL *channel, int stream_id, char *buf, size_t buflen);
int swoole_libssh2_channel_write_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL *channel, int stream_id, const char *buf, size_t buflen);
int swoole_libssh2_channel_eof(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL *channel);
int swoole_libssh2_channel_close(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL *channel);
int swoole_libssh2_channel_send_eof(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL *channel);
int swoole_libssh2_channel_get_exit_status(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL *channel);
int swoole_libssh2_channel_request_pty_size_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL *channel, int width, int height, int width_px, int height_px);
LIBSSH2_LISTENER *swoole_libssh2_channel_forward_listen_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session, const char *host, int port, int *bound_port, int queue_maxsize);
LIBSSH2_CHANNEL *swoole_libssh2_channel_forward_accept(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_LISTENER *listener);
int swoole_libssh2_channel_forward_cancel(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_LISTENER *listener);
LIBSSH2_CHANNEL *swoole_libssh2_channel_direct_tcpip_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session, const char *host, int port, const char *shost, int sport);
int swoole_libssh2_sftp_fstat_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP_HANDLE *handle, LIBSSH2_SFTP_ATTRIBUTES *attrs, int setstat);
int swoole_libssh2_sftp_stat_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP *sftp, const char *path, unsigned int path_len, int stat_type, LIBSSH2_SFTP_ATTRIBUTES *attrs);
int swoole_libssh2_sftp_symlink_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP *sftp, const char *path, unsigned int path_len, char *target, unsigned int target_len, int link_type);
LIBSSH2_SFTP_HANDLE *swoole_libssh2_sftp_open_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP *sftp, const char *filename, unsigned int filename_len, unsigned long flags, long mode, int open_type);
int swoole_libssh2_sftp_readdir_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP_HANDLE *handle, char *buffer, size_t buffer_maxlen, char *longentry, size_t longentry_maxlen, LIBSSH2_SFTP_ATTRIBUTES *attrs);
int swoole_libssh2_sftp_tell(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP_HANDLE *handle);
int swoole_libssh2_sftp_read(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP_HANDLE *handle, char *buffer, size_t buffer_maxlen);
int swoole_libssh2_sftp_write(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP_HANDLE *handle, const char *buffer, size_t count);
LIBSSH2_SFTP *swoole_libssh2_sftp_init(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session);
LIBSSH2_CHANNEL *swoole_libssh2_scp_recv(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session, const char *path, struct stat *sb);
LIBSSH2_CHANNEL *swoole_libssh2_scp_send_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session, const char *path, int mode, size_t size, long mtime, long atime);
int swoole_libssh2_sftp_close_handle(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP_HANDLE *handle);
int swoole_libssh2_sftp_unlink_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP *sftp, const char *filename, unsigned int filename_len);
int swoole_libssh2_sftp_rename_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP *sftp, const char *source_filename, unsigned int source_filename_len, const char *dest_filename, unsigned int dest_filename_len, long flags);
int swoole_libssh2_sftp_mkdir_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP *sftp, const char *path, unsigned int path_len, long mode);
int swoole_libssh2_sftp_rmdir_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP *sftp, const char *path, unsigned int path_len);
int swoole_libssh2_agent_connect(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_AGENT *agent);
int swoole_libssh2_agent_list_identities(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_AGENT *agent);
int swoole_libssh2_agent_get_identity(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_AGENT *agent, struct libssh2_agent_publickey **store, struct libssh2_agent_publickey *prev);
int swoole_libssh2_agent_userauth(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_AGENT *agent, const char *username, struct libssh2_agent_publickey *identity);
int swoole_libssh2_agent_disconnect(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_AGENT *agent);
char *swoole_libssh2_userauth_list(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session, const char *username, unsigned int username_len);
int swoole_libssh2_userauth_password_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session, const char *username, unsigned int username_len, const char *password, unsigned int password_len, LIBSSH2_PASSWD_CHANGEREQ_FUNC((*passwd_change_cb)));
int swoole_libssh2_userauth_publickey_fromfile_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session, const char *username, unsigned int username_len, const char *publickey, const char *privatekey, const char *passphrase);
int swoole_libssh2_userauth_publickey_frommemory(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session, const char *username, size_t username_len, const char *publickeyfiledata, size_t publickeyfiledata_len, const char *privatekeyfiledata, size_t privatekeyfiledata_len, const char *passphrase);
int swoole_libssh2_userauth_hostbased_fromfile_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session, const char *username, unsigned int username_len, const char *publickey, const char *privatekey, const char *passphrase, const char *hostname, unsigned int hostname_len, const char *local_username, unsigned int local_username_len);
int swoole_libssh2_userauth_keyboard_interactive_ex(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session, const char *username, unsigned int username_len, LIBSSH2_USERAUTH_KBDINT_RESPONSE_FUNC((*response_callback)));
int swoole_libssh2_userauth_authenticated(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION *session);
#ifdef __cplusplus
}
#endif
#endif
