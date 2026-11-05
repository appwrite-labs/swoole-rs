#include "swoole_server.h"
#include <cassert>
#include <iostream>

int main() {
    swoole_init();
    {
        swoole::Server server(swoole::Server::MODE_BASE);
        server.worker_num = 2;
        assert(server.add_port(SW_SOCK_TCP, "127.0.0.1", 0));
        assert(server.create() == SW_OK);
        server.get_event_worker_pool()->workers = server.workers;
        auto *worker = &server.workers[0];
        auto *other = &server.workers[1];
        worker->pid = other->pid = 79;
        assert(server.get_worker_by_pid(79) == worker);
        worker->pid = 80;
        assert(server.get_worker_by_pid(79) == other);
        other->pid = -7;
        assert(server.get_worker_by_pid(-7) == other);
        assert(server.get_worker_by_pid(79) == nullptr);

        worker->id = 1;
        worker->concurrency = 3;
        worker->request_count = 7;
        worker->response_count = 8;
        worker->dispatch_count = 9;
        server.gs->concurrency = 7;
        const size_t last = SW_SESSION_LIST_SIZE - 1;
        for (size_t index = 0; index < SW_SESSION_LIST_SIZE; ++index) {
            auto *slot = server.get_session(index);
            slot->id = index + 41;
            slot->fd = 23;
            slot->reactor_id = 2;
            slot->reserve_ = 0x5a1234;
        }
        server.get_session(0)->reactor_id = 1;
        server.get_session(last)->reactor_id = 1;
        server.abort_worker(worker);
        assert(worker->concurrency == 0 && server.gs->concurrency == 4);
        assert(worker->request_count == 0 && worker->response_count == 0 && worker->dispatch_count == 0);
        for (size_t index = 0; index < SW_SESSION_LIST_SIZE; ++index) {
            const auto *slot = server.get_session(index);
            assert(slot->id == static_cast<swoole::SessionId>(index + 41));
            assert(slot->reserve_ == 0x5a1234);
            assert(slot->reactor_id == (index == 0 || index == last ? 1 : 2));
            assert(slot->fd == (index == 0 || index == last ? 0 : 23));
        }
        worker->concurrency = 9;
        server.gs->concurrency = 3;
        server.reset_worker_counter(worker);
        assert(worker->concurrency == 0 && server.gs->concurrency == 0);
        server.destroy();
    }
    swoole_clean();
    std::cout << "worker-retirement-ok\n";
}
