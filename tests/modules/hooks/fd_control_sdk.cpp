#include "swoole_socket_impl.h"
#include "swoole_coroutine_api.h"
#include <cassert>
#include <cstdio>
#include <stdexcept>
#include <sys/socket.h>
#include <pthread.h>
#include <atomic>
#include <sched.h>

using namespace swoole;
using namespace swoole::coroutine;

struct PollFailure {};
struct PollSocket : SocketImpl {
    bool throws = false;
    int calls = 0;
    int *drops;
    explicit PollSocket(int *drops) : SocketImpl(SW_SOCK_TCP), drops(drops) {}
    bool poll(EventType, double = 0) override {
        assert(get_timeout(SW_TIMEOUT_DNS) == 0.125);
        assert(get_timeout(SW_TIMEOUT_READ) == 0.125);
        assert(get_timeout(SW_TIMEOUT_WRITE) == 0.125);
        struct Local { int *drops; ~Local() { ++*drops; } } local{drops};
        ++calls;
        if (throws) throw PollFailure{};
        return false;
    }
};

struct CancelContext {
    int fd;
    bool fake;
    int drops = 0;
    std::atomic<bool> entered{false};
};
static void *cancel_poll(void *raw) {
    auto *context = static_cast<CancelContext *>(raw);
    struct Local { int *drops; ~Local() { ++*drops; } } local{&context->drops};
    context->entered = true;
    if (context->fake) {
        pollfd fd{context->fd, POLLIN, 0};
        swoole_coroutine_poll_fake(&fd, 1, 100000);
    } else {
        swoole_coroutine_socket_wait_event(context->fd, SW_EVENT_READ, 100);
    }
    return nullptr;
}
static void cancellation_points_stay_native() {
    for (bool fake : {false, true}) {
        int pair[2];
        assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
        CancelContext context{pair[0], fake};
        pthread_t thread;
        assert(pthread_create(&thread, nullptr, cancel_poll, &context) == 0);
        while (!context.entered) sched_yield();
        assert(pthread_cancel(thread) == 0);
        void *result = nullptr;
        assert(pthread_join(thread, &result) == 0);
        assert(result == PTHREAD_CANCELED && context.drops == 1);
        ::close(pair[0]);
        ::close(pair[1]);
    }
}

int main() {
    swoole_init();
    bool ran = false;
    coroutine::run([&](void *) {
        ran = true;
        const int fd = swoole_coroutine_socket(AF_INET, SOCK_STREAM, 0);
        assert(fd >= 0);
        auto *socket = swoole_coroutine_get_socket_object(fd);
        assert(socket);
        assert(swoole_coroutine_socket_set_timeout(fd, SO_RCVTIMEO, 0.25) == 0);
        assert(socket->get_timeout(SW_TIMEOUT_READ) == 0.25);
        assert(swoole_coroutine_socket_set_timeout(fd, SO_SNDTIMEO, 0.75) == 0);
        assert(socket->get_timeout(SW_TIMEOUT_WRITE) == 0.75);
        assert(swoole_coroutine_socket_set_connect_timeout(fd, 0.5) == 0);
        assert(socket->get_timeout(SW_TIMEOUT_DNS) == 0.5);
        assert(socket->get_timeout(SW_TIMEOUT_CONNECT) == 0.5);
        errno = 0;
        assert(swoole_coroutine_socket_set_timeout(fd, -1, 2) == -1);
        assert(errno == EINVAL);
        pollfd polled{fd, POLLIN | POLLOUT | POLLPRI, POLLERR};
        assert(swoole_coroutine_poll_fake(&polled, 1, 125) == 1);
        assert(polled.revents == (POLLIN | POLLOUT | POLLERR));
        assert(socket->get_timeout(SW_TIMEOUT_READ) == 0.125);
        assert(swoole_coroutine_close(fd) == 0);

        int pair[2];
        assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
        assert(swoole_coroutine_socket_wait_event(pair[0], SW_EVENT_READ, 0) == SW_ERR);
        assert(::write(pair[1], "x", 1) == 1);
        assert(swoole_coroutine_socket_wait_event(pair[0], SW_EVENT_READ, 0) == SW_OK);
        ::close(pair[0]);
        ::close(pair[1]);

        int drops = 0;
        PollSocket custom(&drops);
        custom.set_timeout(4, SW_TIMEOUT_READ);
        custom.set_timeout(8, SW_TIMEOUT_WRITE);
        assert(!swr_v1_hook_socket_insert(custom.get_fd(), &custom, true));
        assert(swoole_coroutine_socket_wait_event(custom.get_fd(), SW_EVENT_READ, 0.125) == SW_ERR);
        assert(custom.get_timeout(SW_TIMEOUT_DNS) == 4);
        assert(custom.get_timeout(SW_TIMEOUT_WRITE) == 4);
        assert(drops == 1 && custom.calls == 1);
        custom.throws = true;
        bool caught = false;
        try { swoole_coroutine_socket_wait_event(custom.get_fd(), SW_EVENT_READ, 0.125); }
        catch (const PollFailure &) { caught = true; }
        assert(caught && drops == 2 && custom.calls == 2);
        // Upstream restores only after normal virtual return; a throw keeps the temporary value.
        assert(custom.get_timeout(SW_TIMEOUT_READ) == 0.125);
        assert(swr_v1_hook_socket_remove(custom.get_fd()) == &custom);
        custom.close();
    });
    assert(ran);
    cancellation_points_stay_native();
    swoole_clean();
    std::puts("actual fd controls, native poll continuation, C++ exception/destruction and pthread cancellation passed");
}
