#include <openssl/ssl.h>
#include <openssl/err.h>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <initializer_list>
#include <cerrno>
#include <csignal>
#include <fcntl.h>
#include <sys/socket.h>
#include <unistd.h>

static int injected_error, shutdown_calls, warnings;
static int last_shutdown_errno, last_error_class;
static unsigned long last_error_queue;
static int observed_shutdown(SSL *ssl) {
    ++shutdown_calls;
    int result = injected_error ? -1 : SSL_shutdown(ssl);
    last_shutdown_errno = errno;
    return result;
}
static int observed_error(const SSL *ssl, int result) {
    last_error_class = injected_error ? injected_error : SSL_get_error(ssl, result);
    last_error_queue = ERR_peek_error();
    return last_error_class;
}
// Upstream's Socket::ssl_get_error_reason (Rust's in reactor builds), which the methods log with.
static const char *ssl_get_error_reason(int *reason) {
    unsigned long error = ERR_get_error();
    if (reason) {
        *reason = ERR_GET_REASON(error);
    }
    return ERR_reason_error_string(error);
}
static int null_warnings;
static void record_warning(int, const char *message) {
    ++warnings;
    null_warnings += message == nullptr;
}
#define swoole_trace(...) ((void) 0)
#define swoole_warning(format, reason, message) record_warning(reason, message)
class Socket {
  public:
    SSL *ssl;
    bool ssl_closed_ = false, ssl_quiet_shutdown = false;
    bool ssl_want_read = false, ssl_want_write = false;
    void ssl_clear_error() { ERR_clear_error(); ssl_want_read = ssl_want_write = false; }
    bool ssl_shutdown();
    void ssl_close();
};
#define SSL_shutdown observed_shutdown
#define SSL_get_error observed_error
// ACTUAL_SOCKET_METHODS
#undef SSL_shutdown
#undef SSL_get_error

static int checks, failures;
static void check(bool ok, const char *name) {
    ++checks;
    if (!ok) { ++failures; std::fprintf(stderr, "FAIL %s\n", name); }
}
struct Pair {
    SSL_CTX *server_ctx = SSL_CTX_new(TLS_server_method());
    SSL_CTX *client_ctx = SSL_CTX_new(TLS_client_method());
    SSL *server = nullptr, *client = nullptr;
    int fds[2] = {-1, -1};
    Pair(const char *cert, const char *key, bool native_socket = false) {
        SSL_CTX_set_max_proto_version(server_ctx, TLS1_2_VERSION);
        SSL_CTX_set_max_proto_version(client_ctx, TLS1_2_VERSION);
        SSL_CTX_set_verify(client_ctx, SSL_VERIFY_NONE, nullptr);
        if (SSL_CTX_use_certificate_file(server_ctx, cert, SSL_FILETYPE_PEM) != 1 ||
            SSL_CTX_use_PrivateKey_file(server_ctx, key, SSL_FILETYPE_PEM) != 1) std::abort();
        server = SSL_new(server_ctx); client = SSL_new(client_ctx);
        if (native_socket) {
            if (socketpair(AF_UNIX, SOCK_STREAM, 0, fds) != 0) std::abort();
            for (int fd : fds) if (fcntl(fd, F_SETFL, O_NONBLOCK) != 0) std::abort();
            SSL_set_fd(server, fds[0]); SSL_set_fd(client, fds[1]);
        } else {
            BIO *left, *right;
            if (BIO_new_bio_pair(&left, 0, &right, 0) != 1) std::abort();
            SSL_set_bio(server, left, left); SSL_set_bio(client, right, right);
        }
        SSL_set_accept_state(server); SSL_set_connect_state(client);
        for (int i = 0; i < 1000 && (SSL_in_init(server) || SSL_in_init(client)); ++i) {
            SSL_do_handshake(client); SSL_do_handshake(server);
        }
        if (SSL_in_init(server) || SSL_in_init(client)) std::abort();
        ERR_clear_error();
    }
    ~Pair() { SSL_free(server); SSL_free(client); SSL_CTX_free(server_ctx); SSL_CTX_free(client_ctx);
        for (int fd : fds) if (fd >= 0) ::close(fd);
    }
};
int main(int argc, char **argv) {
    if (argc != 3) return 2;
    std::signal(SIGPIPE, SIG_IGN);
    {
        Pair pair(argv[1], argv[2]); Socket socket{pair.server};
        char received[5] = {};
        check(SSL_write(pair.server, "bytes", 5) == 5, "payload write");
        check(socket.ssl_shutdown(), "one-way shutdown succeeds without peer wait");
        check(SSL_read(pair.client, received, 5) == 5 && std::string(received, 5) == "bytes", "payload precedes alert");
        int n = SSL_read(pair.client, received, 1);
        check(n == 0 && SSL_get_error(pair.client, n) == SSL_ERROR_ZERO_RETURN, "strict close_notify");
        int calls = shutdown_calls;
        check(!socket.ssl_shutdown() && shutdown_calls == calls, "one shutdown only");
        socket.ssl_close(); pair.server = nullptr;
        check(!socket.ssl && shutdown_calls == calls, "close frees without duplicate shutdown");
    }
    {
        Pair pair(argv[1], argv[2]); Socket socket{pair.server}; socket.ssl_quiet_shutdown = true;
        check(socket.ssl_shutdown(), "quiet shutdown succeeds");
        check(BIO_ctrl_pending(SSL_get_rbio(pair.client)) == 0, "quiet shutdown emits no alert");
    }
    {
        Pair pair(argv[1], argv[2]); Socket socket{pair.server}; char byte;
        check(SSL_shutdown(pair.client) == 0, "peer first sends alert without waiting");
        int n = SSL_read(pair.server, &byte, 1);
        check(n == 0 && SSL_get_error(pair.server, n) == SSL_ERROR_ZERO_RETURN, "peer alert received");
        check(socket.ssl_shutdown(), "peer first acknowledged");
        check(SSL_shutdown(pair.client) == 1, "peer first completes");
    }
    {
        SSL_CTX *ctx = SSL_CTX_new(TLS_server_method()); Socket socket{SSL_new(ctx)};
        SSL_set_accept_state(socket.ssl); int calls = shutdown_calls;
        check(!socket.ssl_shutdown() && shutdown_calls == calls, "unfinished handshake not shut down");
        socket.ssl_close(); check(!socket.ssl, "unfinished handshake freed"); SSL_CTX_free(ctx);
    }
    {
        Pair pair(argv[1], argv[2]); Socket socket{pair.server};
        check(SSL_shutdown(pair.server) == 0, "pending shutdown sends alert");
        ERR_clear_error();
        check(!socket.ssl_shutdown(), "real nonblocking WANT_READ reported without waiting");
        check(socket.ssl_want_read && !socket.ssl_closed_, "pending read remains retryable");
    }
    {
        Pair pair(argv[1], argv[2]); Socket socket{pair.server};
        std::string data(16384, 'x'); int result = 1, attempts = 0;
        while (result > 0 && ++attempts < 100) result = SSL_write(pair.server, data.data(), data.size());
        check(result < 0 && SSL_get_error(pair.server, result) == SSL_ERROR_WANT_WRITE,
              "real bounded BIO write backpressure");
        ERR_clear_error();
        check(!socket.ssl_shutdown(), "real pending write shutdown reports incomplete/error");
        check(socket.ssl_want_write && !socket.ssl_closed_, "pending write remains retryable");
    }
    {
        Pair pair(argv[1], argv[2]); Socket socket{pair.server};
        // Fill only the transport BIO: no unfinished application SSL_write.
        std::string filler(32768, 'x'); int bytes = 0, n;
        while ((n = BIO_write(SSL_get_wbio(pair.server), filler.data(), filler.size())) > 0) bytes += n;
        check(bytes > 0 && BIO_should_retry(SSL_get_wbio(pair.server)), "alert transport full");
        check(!socket.ssl_shutdown() && socket.ssl_want_write && !socket.ssl_closed_, "alert write remains retryable");
        int drained = 0;
        while ((n = BIO_read(SSL_get_rbio(pair.client), &filler[0], filler.size())) > 0) drained += n;
        check(drained == bytes, "transport capacity released");
        check(socket.ssl_shutdown(), "pending alert retries after peer drains");
        char byte; n = SSL_read(pair.client, &byte, 1);
        check(n == 0 && SSL_get_error(pair.client, n) == SSL_ERROR_ZERO_RETURN, "retried alert reaches peer");
    }
    {
        // A completed handshake followed by peer transport death is distinct
        // from alert backpressure: the real write fails with no OpenSSL reason.
        Pair pair(argv[1], argv[2], true); Socket socket{pair.server};
        check(::close(pair.fds[1]) == 0, "peer transport closes after handshake");
        pair.fds[1] = -1; int before = warnings, null_before = null_warnings;
        check(!socket.ssl_shutdown(), "closed peer cannot receive close_notify");
        check(last_error_class == SSL_ERROR_SYSCALL && last_shutdown_errno == EPIPE && (last_error_queue == 0 || (ERR_GET_LIB(last_error_queue) == ERR_LIB_SYS &&
               ERR_GET_REASON(last_error_queue) == EPIPE)),
              "real closed-peer shutdown is EPIPE with only empty/system error queue");
        check(socket.ssl_closed_ && !socket.ssl_want_read && !socket.ssl_want_write,
              "broken pipe is terminal rather than readiness retry");
        check(warnings == before + 1, "fatal peer write remains reported");
        check(null_warnings == null_before, "real peer failure diagnostic never formats a null string");
        int calls = shutdown_calls;
        check(!socket.ssl_shutdown() && shutdown_calls == calls, "terminal broken pipe is not retried");
        std::printf("peer_closed_class=%d errno=%d queue=%lu warnings=%d\n",
                    last_error_class, last_shutdown_errno, last_error_queue, warnings - before);
    }
    for (int error : {SSL_ERROR_WANT_READ, SSL_ERROR_WANT_WRITE, SSL_ERROR_SSL, SSL_ERROR_SYSCALL, SSL_ERROR_ZERO_RETURN}) {
        Pair pair(argv[1], argv[2]); Socket socket{pair.server};
        injected_error = error; ERR_clear_error(); int before = warnings;
        bool ok = socket.ssl_shutdown();
        check(ok == (error == SSL_ERROR_ZERO_RETURN), "negative native result classified without queued error");
        check(warnings == before + (error != SSL_ERROR_ZERO_RETURN), "incomplete/error shutdown reported");
        check(BIO_ctrl_pending(SSL_get_rbio(pair.client)) == 0, "injected failure never counted as delivered alert");
        injected_error = 0;
    }
    check(null_warnings == 0, "all terminal and injected diagnostics have a non-null message");
    std::printf("{\"checks\":%d,\"failures\":%d,\"shutdown_calls\":%d}\n", checks, failures, shutdown_calls);
    return failures ? 1 : 0;
}
