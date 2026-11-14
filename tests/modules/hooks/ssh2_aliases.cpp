// Compare the pinned convenience macros with the Rust-owned operations against
// the installed SDK's real declarations; fake only the underlying I/O results.
#include <cassert>
#include <cstdio>
#include <cstring>
#include <functional>
#include <libssh2.h>
#include <libssh2_sftp.h>

static unsigned calls;
static LIBSSH2_CHANNEL *channel_value = reinterpret_cast<LIBSSH2_CHANNEL *>(0x1000);
static LIBSSH2_SFTP_HANDLE *handle_value = reinterpret_cast<LIBSSH2_SFTP_HANDLE *>(0x2000);
static void text(const char *actual, const char *expected) { assert(std::strcmp(actual, expected) == 0); }

extern "C" {
int libssh2_session_block_directions(LIBSSH2_SESSION *) { return 0; }
int libssh2_session_last_errno(LIBSSH2_SESSION *) { return 0; }
int libssh2_session_disconnect_ex(LIBSSH2_SESSION *, int reason, const char *description, const char *lang) {
    assert(reason == SSH_DISCONNECT_BY_APPLICATION); text(description, "bye"); text(lang, ""); ++calls; return 7;
}
LIBSSH2_CHANNEL *libssh2_channel_open_ex(LIBSSH2_SESSION *, const char *kind, unsigned kind_len,
                                       unsigned window, unsigned packet, const char *message, unsigned message_len) {
    text(kind, "session"); assert(kind_len == 7 && window == LIBSSH2_CHANNEL_WINDOW_DEFAULT);
    assert(packet == LIBSSH2_CHANNEL_PACKET_DEFAULT && message == nullptr && message_len == 0);
    ++calls; return channel_value;
}
int libssh2_channel_process_startup(LIBSSH2_CHANNEL *, const char *request, unsigned request_len,
                                  const char *message, unsigned message_len) {
    if (std::strcmp(request, "shell") == 0) {
        assert(request_len == 5 && message == nullptr && message_len == 0);
    } else {
        text(request, "exec"); assert(request_len == 4); text(message, "command"); assert(message_len == 7);
    }
    ++calls; return 7;
}
ssize_t libssh2_channel_read_ex(LIBSSH2_CHANNEL *, int stream, char *, size_t length) {
    assert(stream == 0 && length == 17); ++calls; return 7;
}
ssize_t libssh2_channel_write_ex(LIBSSH2_CHANNEL *, int stream, const char *, size_t length) {
    assert(stream == 0 && length == 19); ++calls; return 7;
}
LIBSSH2_CHANNEL *libssh2_channel_direct_tcpip_ex(LIBSSH2_SESSION *, const char *host, int port,
                                               const char *origin, int origin_port) {
    text(host, "localhost"); text(origin, "127.0.0.1"); assert(port == 42 && origin_port == 22);
    ++calls; return channel_value;
}
int libssh2_sftp_fstat_ex(LIBSSH2_SFTP_HANDLE *, LIBSSH2_SFTP_ATTRIBUTES *, int setstat) {
    assert(setstat == 0); ++calls; return 7;
}
LIBSSH2_SFTP_HANDLE *libssh2_sftp_open_ex(LIBSSH2_SFTP *, const char *path, unsigned length,
                                        unsigned long flags, long mode, int kind) {
    text(path, "path"); assert(length == 4);
    if (kind == LIBSSH2_SFTP_OPENFILE) { assert(flags == 23 && mode == 0640); }
    else { assert(kind == LIBSSH2_SFTP_OPENDIR && flags == 0 && mode == 0); }
    ++calls; return handle_value;
}
int libssh2_sftp_readdir_ex(LIBSSH2_SFTP_HANDLE *, char *, size_t length, char *longentry,
                          size_t longentry_length, LIBSSH2_SFTP_ATTRIBUTES *) {
    assert(length == 31 && longentry == nullptr && longentry_length == 0); ++calls; return 7;
}
int libssh2_sftp_close_handle(LIBSSH2_SFTP_HANDLE *) { ++calls; return 7; }
int libssh2_sftp_unlink_ex(LIBSSH2_SFTP *, const char *path, unsigned length) {
    text(path, "path"); assert(length == 4); ++calls; return 7;
}
int libssh2_sftp_rename_ex(LIBSSH2_SFTP *, const char *source, unsigned source_len,
                         const char *destination, unsigned destination_len, long flags) {
    text(source, "from"); text(destination, "to"); assert(source_len == 4 && destination_len == 2);
    assert(flags == (LIBSSH2_SFTP_RENAME_OVERWRITE | LIBSSH2_SFTP_RENAME_ATOMIC | LIBSSH2_SFTP_RENAME_NATIVE));
    ++calls; return 7;
}
int libssh2_sftp_mkdir_ex(LIBSSH2_SFTP *, const char *path, unsigned length, long mode) {
    text(path, "path"); assert(length == 4 && mode == 0750); ++calls; return 7;
}
int libssh2_sftp_rmdir_ex(LIBSSH2_SFTP *, const char *path, unsigned length) {
    text(path, "path"); assert(length == 4); ++calls; return 7;
}
int libssh2_userauth_publickey_fromfile_ex(LIBSSH2_SESSION *, const char *username, unsigned length,
                                          const char *publickey, const char *privatekey, const char *passphrase) {
    text(username, "user"); assert(length == 4); text(publickey, "pub"); text(privatekey, "priv"); text(passphrase, "pass");
    ++calls; return 7;
}
int libssh2_userauth_keyboard_interactive_ex(LIBSSH2_SESSION *, const char *username, unsigned length,
                                            LIBSSH2_USERAUTH_KBDINT_RESPONSE_FUNC((*callback))) {
    text(username, "user"); assert(length == 4 && callback == nullptr); ++calls; return 7;
}
}

#ifdef SWR_HYBRID
#include "swoole_rs_ssh2.h"
static bool wait(LIBSSH2_SESSION *, int) { assert(false && "no EAGAIN in alias argument probe"); return false; }
static const swr_v1_ssh2_ops ops = {wait};
static const swr_v1_ssh2_ops *ssh2_bound(LIBSSH2_SESSION *) { return &ops; }
#else
static int ssh2_async_call(LIBSSH2_SESSION *, const std::function<int(void)> &call) { return call(); }
template <typename T> static T *ssh2_async_call_ex(LIBSSH2_SESSION *, const std::function<T *(void)> &call) { return call(); }
#endif
#include "php_swoole_ssh2_hook.h"

int main() {
    auto *session = reinterpret_cast<LIBSSH2_SESSION *>(0x3000);
    auto *sftp = reinterpret_cast<LIBSSH2_SFTP *>(0x4000);
    char buffer[64] = {};
    LIBSSH2_SFTP_ATTRIBUTES attrs = {};
    assert(libssh2_session_disconnect(session, "bye") == 7);
    assert(libssh2_channel_open_session(session) == channel_value);
    assert(libssh2_channel_shell(channel_value) == 7);
    assert(libssh2_channel_exec(channel_value, "command\0ignored") == 7);
    assert(libssh2_channel_read(channel_value, buffer, 17) == 7);
    assert(libssh2_channel_write(channel_value, buffer, 19) == 7);
    assert(libssh2_channel_direct_tcpip(session, "localhost", 42) == channel_value);
    assert(libssh2_sftp_fstat(handle_value, &attrs) == 7);
    assert(libssh2_sftp_open(sftp, "path", 23, 0640) == handle_value);
    assert(libssh2_sftp_opendir(sftp, "path") == handle_value);
    assert(libssh2_sftp_readdir(handle_value, buffer, 31, &attrs) == 7);
    assert(libssh2_sftp_close(handle_value) == 7);
    assert(libssh2_sftp_unlink(sftp, "path") == 7);
    assert(libssh2_sftp_rename(sftp, "from", "to") == 7);
    assert(libssh2_sftp_mkdir(sftp, "path", 0750) == 7);
    assert(libssh2_sftp_rmdir(sftp, "path") == 7);
    assert(libssh2_userauth_publickey_fromfile(session, "user", "pub", "priv", "pass") == 7);
    assert(libssh2_userauth_keyboard_interactive(session, "user", nullptr) == 7);
    assert(calls == 18);
    std::puts("aliases=18");
}
