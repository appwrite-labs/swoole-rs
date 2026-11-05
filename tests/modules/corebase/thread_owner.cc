#include "swoole_thread.h"
#include <atomic>
#include <cassert>
#include <condition_variable>
#include <iostream>
#include <future>
#include <chrono>
#include <mutex>
#include <stdexcept>
#include <type_traits>
#include <utility>

using swoole::Thread;
static_assert(!std::is_copy_constructible<Thread>::value, "Thread is move-only");
static_assert(!std::is_copy_assignable<Thread>::value, "Thread is move-only");
static_assert(std::is_nothrow_default_constructible<Thread>::value, "default construction cannot allocate");
static_assert(std::is_nothrow_move_constructible<Thread>::value, "move construction cannot throw");
static_assert(std::is_nothrow_move_assignable<Thread>::value, "move assignment cannot throw");

struct Gate {
    std::mutex mutex;
    std::condition_variable condition;
    bool entered = false, released = false, finished = false;
    void enter() {
        std::unique_lock<std::mutex> guard(mutex);
        entered = true; condition.notify_all();
        condition.wait(guard, [&] { return released; });
    }
    void wait_entered() {
        std::unique_lock<std::mutex> guard(mutex);
        condition.wait(guard, [&] { return entered; });
    }
    void release() {
        std::lock_guard<std::mutex> guard(mutex);
        released = true; condition.notify_all();
    }
    void finish() {
        std::lock_guard<std::mutex> guard(mutex);
        finished = true; condition.notify_all();
    }
    void wait_finished() {
        std::unique_lock<std::mutex> guard(mutex);
        condition.wait(guard, [&] { return finished; });
    }
};

struct Callable {
    static std::atomic<int> copies, destroyed, called;
    static bool fail_copy;
    Callable() = default;
    Callable(const Callable &) {
        if (++copies == 2 && fail_copy) throw std::runtime_error("copy");
    }
    ~Callable() { ++destroyed; }
    void operator()() && { ++called; }
    void operator()() & { std::abort(); }
    static void reset() { copies = 0; destroyed = 0; called = 0; fail_copy = false; }
};
std::atomic<int> Callable::copies{0}, Callable::destroyed{0}, Callable::called{0};
bool Callable::fail_copy = false;

struct PthreadExit {
    static std::atomic<int> destroyed;
    ~PthreadExit() { ++destroyed; }
    void operator()() && { pthread_exit(nullptr); }
};
std::atomic<int> PthreadExit::destroyed{0};

static void join_error(Thread &thread) {
    try { thread.join(); std::abort(); }
    catch (const std::system_error &error) {
        assert(error.code().value() == EINVAL);
        std::cout << "join error " << error.code().category().name() << ": " << error.what() << '\n';
    }
}
static void detach_error(Thread &thread) {
    try { thread.detach(); std::abort(); }
    catch (const std::system_error &error) {
        assert(error.code().value() == EINVAL);
        std::cout << "detach error " << error.code().category().name() << ": " << error.what() << '\n';
    }
}

static const Thread constant_owner;
static Thread *termination_owner = nullptr;
static pthread_t termination_id{};

int main(int argc, char **argv) {
    if (argc > 1) {
        std::set_terminate([] {
            if (termination_owner) assert(termination_owner->get_id() == termination_id);
            std::cout << "terminate\n" << std::flush; std::_Exit(60);
        });
        const std::string mode = argv[1];
        if (mode == "body-throw") {
            Thread thread; thread.start([] { throw std::runtime_error("body"); }); thread.join();
        } else {
            Thread first; first.start([] {});
            termination_owner = &first; termination_id = first.get_id();
            if (mode == "occupied-start") first.start([] {});
            else if (mode == "self-move") { Thread &alias = first; first = std::move(alias); }
            else if (mode == "occupied-move") {
                Thread second; second.start([] {}); first = std::move(second);
            } else assert(mode == "joinable-dtor");
        }
        std::abort();
    }
    assert(!constant_owner.joinable() && !constant_owner.is_alive() && constant_owner.get_exit_status() == 0);
    Thread empty;
    assert(!empty.joinable() && !empty.is_alive() && empty.get_exit_status() == 0 && empty.get_id() == pthread_t{});
    join_error(empty); detach_error(empty);
    {
        Thread owner; Gate gate; pthread_t observed{};
        owner.start([&] { observed = pthread_self(); owner.enter(); gate.enter(); owner.exit(37); });
        gate.wait_entered();
        assert(owner.joinable() && owner.is_alive() && owner.get_exit_status() == 0);
        assert(owner.get_id() != pthread_t{} && pthread_equal(owner.get_id(), observed));
        gate.release(); owner.join();
        assert(!owner.joinable() && !owner.is_alive() && owner.get_exit_status() == 37);
        join_error(owner); detach_error(owner);
    }
    {
        Thread source; Gate gate;
        source.start([&] { source.enter(); gate.enter(); source.exit(41); });
        gate.wait_entered();
        const auto id = source.get_id();
        Thread moved(std::move(source));
        assert(!source.joinable() && moved.joinable() && moved.get_id() == id);
        assert(moved.is_alive() && source.is_alive());
        gate.release(); moved.join();
        // Native's generated move copies status; the callable still names source.
        assert(source.get_exit_status() == 41 && !source.is_alive());
        assert(moved.get_exit_status() == 0 && moved.is_alive());
        Thread assigned; assigned = std::move(moved);
        assert(!assigned.joinable() && assigned.is_alive() && assigned.get_exit_status() == 0);
    }
    {
        Thread owner; Gate gate;
        owner.start([&] { owner.enter(); gate.enter(); owner.exit(43); gate.finish(); });
        gate.wait_entered(); owner.detach(); assert(!owner.joinable());
        gate.release(); gate.wait_finished();
        assert(!owner.is_alive() && owner.get_exit_status() == 43);
    }
    Callable::reset();
    {
        Callable callable; Thread owner; owner.start(callable); owner.join();
        assert(Callable::copies == 2 && Callable::called == 1 && Callable::destroyed == 2);
    }
    assert(Callable::destroyed == 3);
    Callable::reset(); Callable::fail_copy = true;
    {
        Callable callable; Thread owner;
        try { owner.start(callable); std::abort(); }
        catch (const std::runtime_error &error) { assert(std::string(error.what()) == "copy"); }
        assert(!owner.joinable() && Callable::copies == 2 && Callable::called == 0 && Callable::destroyed == 1);
    }
    assert(Callable::destroyed == 2);
    {
        PthreadExit callable; Thread owner; owner.start(callable); owner.join();
        assert(PthreadExit::destroyed == 2);
    }
    assert(PthreadExit::destroyed == 3);
    {
        Thread owner; Gate gate;
        owner.start([&] {
            gate.enter();
            try { owner.join(); std::abort(); }
            catch (const std::system_error &error) { assert(error.code().value() == EDEADLK); std::cout << "self join error " << error.code().category().name() << ": " << error.what() << '\n'; }
            gate.finish();
        });
        gate.wait_entered(); gate.release(); gate.wait_finished(); owner.join();
    }
    {
        Thread owner; Gate worker;
        owner.start([&] { worker.enter(); });
        worker.wait_entered();
        std::atomic<bool> joining{false}, unwound{false};
        std::thread joiner([&] {
            struct Unwound { std::atomic<bool> &flag; ~Unwound() { flag = true; } } guard{unwound};
            joining = true;
            owner.join();
        });
        while (!joining) std::this_thread::yield();
        assert(pthread_cancel(joiner.native_handle()) == 0);
        joiner.join();
        assert(unwound && owner.joinable());
        worker.release(); owner.join();
        assert(!owner.joinable());
    }
    {
        auto promise = std::make_shared<std::promise<int>>();
        auto future = promise->get_future();
        std::atomic<int> destroyed{0};
        Thread owner;
        owner.start([promise, &destroyed] {
            struct Marker { std::atomic<int> &flag; ~Marker() { flag = 1; } };
            thread_local Marker marker{destroyed};
            (void) marker;
            promise->set_value_at_thread_exit(57);
        });
        assert(future.wait_for(std::chrono::seconds(5)) == std::future_status::ready);
        assert(destroyed == 1 && future.get() == 57);
        owner.join();
    }
    char original[64] = {}, name[64] = {};
    assert(swoole_thread_get_name(original, sizeof(original)));
    swoole_set_last_error(941);
    assert(swoole_thread_set_name("swr-owner"));
    assert(swoole_get_last_error() == 941);
    assert(swoole_thread_get_name(name, sizeof(name)) && std::string(name) == "swr-owner");
    std::string oversized(127, 'x');
    assert(!swoole_thread_set_name(oversized.c_str()));
    const int error = swoole_get_last_error();
    assert(error != 0);
    assert(swoole_thread_set_name(original) && swoole_get_last_error() == error);
    assert(swoole_thread_get_native_id() > 0);
    assert(!swoole_thread_id_to_str(std::this_thread::get_id()).empty());
    std::cout << "thread ownership PASS; name error " << error << '\n';
}
