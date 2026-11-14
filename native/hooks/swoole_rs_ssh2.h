#ifndef SWOOLE_RS_SSH2_H
#define SWOOLE_RS_SSH2_H
#include <stdbool.h>
#include <libssh2.h>
#include <libssh2_sftp.h>
#ifdef __cplusplus
extern "C" {
#endif
/* The coroutine ssh2 hook (Rust, crates/ssh2, --with-swoole-ssh2): each
 * blocking libssh2 call the bundled ext/ssh2 makes, retried while libssh2 says
 * EAGAIN, the coroutine waiting on the session's socket in between.
 * php_swoole_ssh2_hook.h calls swoole_<function>(ops, session, arguments...)
 * once no other coroutine is bound to the socket. */
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
int swoole_libssh2_session_disconnect(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION * session, const char * description);
LIBSSH2_CHANNEL * swoole_libssh2_channel_open_session(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION * session);
int swoole_libssh2_channel_shell(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL * channel);
int swoole_libssh2_channel_exec(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL * channel, const char * command);
int swoole_libssh2_channel_read(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL * channel, char * buf, size_t buflen);
int swoole_libssh2_channel_write(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_CHANNEL * channel, const char * buf, size_t buflen);
LIBSSH2_CHANNEL * swoole_libssh2_channel_direct_tcpip(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION * session, const char * host, int port);
int swoole_libssh2_sftp_fstat(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP_HANDLE * handle, LIBSSH2_SFTP_ATTRIBUTES * attrs);
LIBSSH2_SFTP_HANDLE * swoole_libssh2_sftp_open(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP * sftp, const char * filename, unsigned long flags, long mode);
LIBSSH2_SFTP_HANDLE * swoole_libssh2_sftp_opendir(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP * sftp, const char * path);
int swoole_libssh2_sftp_readdir(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP_HANDLE * handle, char * buffer, size_t buffer_maxlen, LIBSSH2_SFTP_ATTRIBUTES * attrs);
int swoole_libssh2_sftp_close(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP_HANDLE * handle);
int swoole_libssh2_sftp_unlink(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP * sftp, const char * filename);
int swoole_libssh2_sftp_rename(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP * sftp, const char * sourcefile, const char * destfile);
int swoole_libssh2_sftp_mkdir(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP * sftp, const char * path, long mode);
int swoole_libssh2_sftp_rmdir(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SFTP * sftp, const char * path);
int swoole_libssh2_userauth_publickey_fromfile(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION * session, const char * username, const char * publickey, const char * privatekey, const char * passphrase);
int swoole_libssh2_userauth_keyboard_interactive(const struct swr_v1_ssh2_ops *ops, LIBSSH2_SESSION *wait_on, LIBSSH2_SESSION * session, const char * username, LIBSSH2_USERAUTH_KBDINT_RESPONSE_FUNC((*response_callback)));
#ifdef PHP_FUNCTION
/* What the bundled ext/ssh2 and crates/phpswoole share: its PHP
 * functions, in crates/phpstubs' order, and its module hooks. */
#define SWR_SSH2_FUNCTIONS(X) \
    X(ssh2_connect) X(ssh2_disconnect) X(ssh2_methods_negotiated) X(ssh2_fingerprint) \
    X(ssh2_auth_none) X(ssh2_auth_password) X(ssh2_auth_pubkey_file) X(ssh2_auth_pubkey) \
    X(ssh2_auth_hostbased_file) X(ssh2_forward_listen) X(ssh2_forward_accept) X(ssh2_shell) \
    X(ssh2_shell_resize) X(ssh2_exec) X(ssh2_tunnel) X(ssh2_scp_recv) \
    X(ssh2_scp_send) X(ssh2_fetch_stream) X(ssh2_send_eof) X(ssh2_sftp) \
    X(ssh2_sftp_rename) X(ssh2_sftp_unlink) X(ssh2_sftp_mkdir) X(ssh2_sftp_rmdir) \
    X(ssh2_sftp_chmod) X(ssh2_sftp_stat) X(ssh2_sftp_lstat) X(ssh2_sftp_symlink) \
    X(ssh2_sftp_readlink) X(ssh2_sftp_realpath) X(ssh2_publickey_init) X(ssh2_publickey_add) \
    X(ssh2_publickey_remove) X(ssh2_publickey_list) X(ssh2_auth_agent)
#define SWR_SSH2_DECLARE(name) PHP_FUNCTION(name);
SWR_SSH2_FUNCTIONS(SWR_SSH2_DECLARE)
int php_swoole_ssh2_minit(int module_number);
int php_swoole_ssh2_mshutdown(void);
void php_swoole_ssh2_minfo(void);
/* Rust (crates/hooks/src/ssh2.rs, built with --with-swoole-ssh2): the ops for
 * session's socket once no other coroutine is bound to it, and the socket's
 * timeout, whole seconds of timeout_ms. */
#ifdef SW_HAVE_SSH2LIB
const struct swr_v1_ssh2_ops *swr_v1_ssh2_bound(LIBSSH2_SESSION *session);
void swr_v1_ssh2_set_socket_timeout(LIBSSH2_SESSION *session, int timeout_ms);
#define ssh2_bound swr_v1_ssh2_bound
#define ssh2_set_socket_timeout swr_v1_ssh2_set_socket_timeout
#endif
#endif
#ifdef __cplusplus
}
#endif
#if defined(__cplusplus) && defined(PHP_FUNCTION)
/* An ext/ssh2 session's abstract: its userspace callbacks and its socket. */
struct php_ssh2_session_data {
    zval *ignore_cb;
    zval *debug_cb;
    zval *macerror_cb;
    zval *disconnect_cb;
    SocketImpl *socket;
};
#endif
#endif
