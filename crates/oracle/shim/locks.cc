// The pinned Mutex, RWLock, SpinLock, Barrier and atomic wait behind C entry points:
// build.rs copies swoole_lock.h and the bodies unchanged into namespace locks_upstream
// (locks_upstream.h, locks_upstream.inc). `exercise`, `shared_barrier` and `spin` are
// the drivers of the former tests/modules/locks/gate.cc, whose other side is now Rust
// (tests/modules/locks/gate.rs). The drivers lock inside assert().
#undef NDEBUG
#include <algorithm>
#include <cassert>
#include <cerrno>
#include <chrono>
#include <climits>
#include <cstdint>
#include <cstdlib>
#include <functional>
#include <iostream>
#include <pthread.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>
#include <vector>

// What the pinned text takes from swoole.h and swoole_memory.h.
using sw_atomic_t = volatile uint32_t;
#define sw_atomic_add_fetch(p, n) __sync_add_and_fetch(p, n)
#define sw_atomic_sub_fetch(p, n) __sync_sub_and_fetch(p, n)
#define sw_atomic_fetch_add(p, n) __sync_fetch_and_add(p, n)
#define sw_atomic_cmp_set(p, o, n) __sync_bool_compare_and_swap(p, o, n)
#define sw_atomic_memory_barrier() __sync_synchronize()
#define SW_LOOP while (true)
#define SW_NUM_BILLION 1000000000
#define SW_NUM_MILLION 1000000
// A shared lock lives in a shared page, as in sw_mem_pool.
struct Pool {
    static constexpr size_t page = 1 << 16;
    void *alloc(size_t n) {
        return n <= page ? mmap(nullptr, page, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANON, -1, 0) : nullptr;
    }
    void free(void *p) {
        munmap(p, page);
    }
};
inline Pool *sw_mem_pool() {
    static Pool pool;
    return &pool;
}
// For the non-futex wait registry (misc.cc's #else branch).
#define sw_spinlock_release(lock) __sync_lock_release(lock)
#define sw_atomic_cpu_pause() std::this_thread::yield()
#define SW_ERROR_MALLOC_FAIL 501
inline void swoole_fatal_error(int, const char *) {
    abort();
}
inline void swoole_set_last_error(int e) {
    errno = e;
}

#include "locks_upstream.h"
#include "locks_upstream.inc"

#define CHECK(x)                                                                                                       \
    do {                                                                                                               \
        if (!(x)) {                                                                                                    \
            std::cerr << "failed: " << #x << " line " << __LINE__ << std::endl;                                        \
            std::abort();                                                                                              \
        }                                                                                                              \
    } while (0)

template<class Mutex, class RW, class Barrier>
std::vector<int> exercise() {
    std::vector<int> results;
    Mutex mutex(false);
    int counter = 0;
    std::vector<std::thread> threads;
    for (int t=0;t<4;t++) threads.emplace_back([&] {
        for(int i=0;i<2000;i++) { assert(mutex.lock()==0); counter++; assert(mutex.unlock()==0); }
    });
    for(auto &t:threads) t.join();
    CHECK(counter==8000);
    CHECK(mutex.lock()==0);
    for(int op: {LOCK_NB, LOCK_NB|LOCK_SH, LOCK_NB|LOCK_EX, -1}) results.push_back(mutex.lock(op,10));
    results.push_back(mutex.lock(LOCK_EX, 5));
    CHECK(results.back()==ETIMEDOUT);
    CHECK(mutex.unlock()==0);
    results.push_back(mutex.lock(LOCK_EX,5)); CHECK(results.back()==0); CHECK(mutex.unlock()==0);
    // Zero/negative timeouts retain blocking semantics, not immediate timeouts.
    for(int timeout: {0,-1,-100}) {
        CHECK(mutex.lock()==0); int result=-99;
        std::thread waiter([&]{result=mutex.lock(LOCK_EX,timeout);if(result==0)assert(mutex.unlock()==0);});
        usleep(2000); CHECK(mutex.unlock()==0); waiter.join(); CHECK(result==0);results.push_back(result);
    }
    RW rw(false);
    CHECK(rw.lock(LOCK_SH)==0);
    int read=-1,write=-1;
    std::thread readers([&]{write=rw.lock(LOCK_EX|LOCK_NB); read=rw.lock(LOCK_SH|LOCK_NB);if(!read)assert(rw.unlock()==0);});
    readers.join();CHECK(read==0);CHECK(write!=0);results.push_back(write);CHECK(rw.unlock()==0);
    CHECK(rw.lock(LOCK_EX)==0);
    int timed=-1;std::thread timed_reader([&]{timed=rw.lock(LOCK_SH,5);});timed_reader.join();CHECK(timed==ETIMEDOUT);results.push_back(timed);CHECK(rw.unlock()==0);
    // Parent/child share the same allocator-backed pthread mutex and payload.
    Mutex shared(true);
    int *payload=(int*)mmap(nullptr,4096,PROT_READ|PROT_WRITE,MAP_SHARED|MAP_ANON,-1,0);CHECK(payload!=MAP_FAILED);
    *payload=0;CHECK(shared.lock()==0);pid_t child=fork();CHECK(child>=0);
    if(!child){if(shared.lock()!=0)_exit(11);*payload=12345;if(shared.unlock()!=0)_exit(12);_exit(0);}
    usleep(2000);CHECK(shared.unlock()==0);int status;CHECK(waitpid(child,&status,0)==child);CHECK(WIFEXITED(status)&&WEXITSTATUS(status)==0);CHECK(*payload==12345);munmap(payload,4096);
    Barrier barrier{};barrier.init(false,4);int slots[4]={0,0,0,0};threads.clear();
    for(int i=0;i<4;i++)threads.emplace_back([&,i]{slots[i]=1;barrier.wait();for(int j:slots)assert(j==1);});
    for(auto&t:threads)t.join();barrier.destroy();
    return results;
}

template<class Barrier>
static void shared_barrier() {
    void *page=mmap(nullptr,4096,PROT_READ|PROT_WRITE,MAP_SHARED|MAP_ANON,-1,0);CHECK(page!=MAP_FAILED);
    auto *barrier=new(page) Barrier{};auto *value=(int*)((char*)page+2048);*value=0;
    barrier->init(true,2);pid_t pid=fork();CHECK(pid>=0);
    if(!pid){*value=9876;barrier->wait();_exit(0);}
    barrier->wait();CHECK(*value==9876);int status;CHECK(waitpid(pid,&status,0)==pid);CHECK(WIFEXITED(status)&&WEXITSTATUS(status)==0);barrier->destroy();munmap(page,4096);
}

template<class Spin>
static std::vector<int> spin() {
    Spin lock(false);
    return {lock.lock(),lock.lock(LOCK_NB),lock.lock(LOCK_EX,3),lock.unlock()};
}

extern "C" {
// `exercise`'s results, up to `capacity` of them; returns how many there are.
size_t swr_oracle_locks_exercise(int *results, size_t capacity) {
    auto all = ::exercise<locks_upstream::Mutex, locks_upstream::RWLock, locks_upstream::Barrier>();
    std::copy_n(all.begin(), std::min(capacity, all.size()), results);
    return all.size();
}
#ifdef HAVE_SPINLOCK
// `spin`'s four results.
void swr_oracle_locks_spin(int *results) {
    auto all = ::spin<locks_upstream::SpinLock>();
    std::copy_n(all.begin(), 4, results);
}
#endif
#ifndef SW_USE_PTHREAD_BARRIER
void swr_oracle_locks_shared_barrier() {
    ::shared_barrier<locks_upstream::Barrier>();
}
#endif
// MutexImpl's (kind 3) or RWLockImpl's size and alignment.
size_t swr_oracle_locks_size(int kind) {
    return kind == 3 ? sizeof(locks_upstream::MutexImpl) : sizeof(locks_upstream::RWLockImpl);
}
size_t swr_oracle_locks_align(int kind) {
    return kind == 3 ? alignof(locks_upstream::MutexImpl) : alignof(locks_upstream::RWLockImpl);
}
int swr_oracle_locks_wait(uint32_t *word, double timeout) {
    return locks_upstream::sw_atomic_futex_wait(word, timeout);
}
int swr_oracle_locks_wake(uint32_t *word, int count) {
    return locks_upstream::sw_atomic_futex_wakeup(word, count);
}
#ifndef HAVE_FUTEX
void swr_oracle_locks_wait_init() {
    locks_upstream::sw_atomic_wait_init();
}
#endif
// The `__sync` builtins behind sw_atomic_t and Swoole\Atomic: add-fetch (2), sub-fetch
// (3) and compare-set (4, 0 or 1) on `word`.
uint32_t swr_oracle_locks_sync32(uint32_t *word, uint32_t operation, uint32_t value, uint32_t expected) {
    switch (operation) {
    case 2:
        return __sync_add_and_fetch(word, value);
    case 3:
        return __sync_sub_and_fetch(word, value);
    default:
        return __sync_bool_compare_and_swap(word, expected, value);
    }
}
int64_t swr_oracle_locks_sync64(int64_t *word, uint32_t operation, int64_t value, int64_t expected) {
    switch (operation) {
    case 2:
        return __sync_add_and_fetch(word, value);
    case 3:
        return __sync_sub_and_fetch(word, value);
    default:
        return __sync_bool_compare_and_swap(word, expected, value);
    }
}
}
