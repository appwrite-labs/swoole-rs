// Two translation units: LOCKS_REFERENCE builds the pinned bodies against the pinned header,
// the other the driver against the Cargo build SDK's header, whose Barrier methods and lock ABI
// are the archive's Rust.
#ifdef LOCKS_REFERENCE
#include "reference.h"
#include "original.inc"
#else
#include "swoole_lock.h"
#include "swoole_rs_locks.h"
#endif
#include <cassert>
#include <iostream>
#include <thread>
#include <vector>
#include <sys/wait.h>

inline int checks;
#define CHECK(x) do { ++checks; if (!(x)) { std::cerr << "failed: " << #x << " line " << __LINE__ << std::endl; std::abort(); } } while(0)

#ifndef LOCKS_REFERENCE
// The hybrid's locks: Swoole\Lock and the server call Rust's lock ABI directly. A shared
// lock lives in a shared page, as upstream's Mutex(true) did in sw_mem_pool.
template<int Kind> struct RustLock {
    static constexpr size_t page=4096;
    void *storage;
    explicit RustLock(bool shared) {
        CHECK(swr_v1_locks_size(Kind)<=page);
        storage=mmap(nullptr,page,PROT_READ|PROT_WRITE,(shared?MAP_SHARED:MAP_PRIVATE)|MAP_ANON,-1,0);CHECK(storage!=MAP_FAILED);
        CHECK(swr_v1_locks_init(storage,Kind,shared)==0);
    }
    ~RustLock() { swr_v1_locks_destroy(storage,Kind);munmap(storage,page); }
    int lock(int operation=LOCK_EX,int timeout=-1) {
        uint32_t capabilities=0;
#ifdef HAVE_MUTEX_TIMEDLOCK
        capabilities|=1;
#endif
#ifdef HAVE_RWLOCK_TIMEDRDLOCK
        capabilities|=2;
#endif
        return swr_v1_locks_lock(storage,Kind,operation,timeout,capabilities);
    }
    int unlock() { return swr_v1_locks_unlock(storage,Kind); }
};
#endif

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

// The reference side's entry points.
namespace entry {
std::vector<int> exercise();
std::vector<int> spin();
void shared_barrier();
size_t size(int kind);
size_t align(int kind);
int wait(uint32_t *word,double timeout);
int wake(uint32_t *word,int count);
void wait_init();
}

#ifdef LOCKS_REFERENCE
namespace entry {
std::vector<int> exercise() { return ::exercise<reference::Mutex,reference::RWLock,reference::Barrier>(); }
#ifdef HAVE_SPINLOCK
std::vector<int> spin() { return ::spin<reference::SpinLock>(); }
#endif
#ifndef SW_USE_PTHREAD_BARRIER
void shared_barrier() { ::shared_barrier<reference::Barrier>(); }
#endif
size_t size(int kind) { return kind==3?sizeof(reference::MutexImpl):sizeof(reference::RWLockImpl); }
size_t align(int kind) { return kind==3?alignof(reference::MutexImpl):alignof(reference::RWLockImpl); }
int wait(uint32_t *word,double timeout) { return reference::sw_atomic_futex_wait(word,timeout); }
int wake(uint32_t *word,int count) { return reference::sw_atomic_futex_wakeup(word,count); }
#ifndef HAVE_FUTEX
void wait_init() { reference::sw_atomic_wait_init(); }
#endif
}
#else
static uint64_t random_state=0x28574a92;
static uint64_t next(){random_state^=random_state<<13;random_state^=random_state>>7;random_state^=random_state<<17;return random_state;}
static void atomics() {
    uint32_t a=0,b=0;int64_t c=0,d=0;
    for(int i=0;i<20000;i++) {
        uint64_t v=next();uint32_t expected=i%3? (uint32_t)next():a;
        CHECK(swr_v1_locks_atomic32(&b,2,v,0)==__sync_add_and_fetch(&a,(uint32_t)v));
        CHECK(swr_v1_locks_atomic32(&b,3,v>>32,0)==__sync_sub_and_fetch(&a,(uint32_t)(v>>32)));
        CHECK(swr_v1_locks_atomic32(&b,4,v,expected)==__sync_bool_compare_and_swap(&a,expected,(uint32_t)v));
        CHECK(swr_v1_locks_atomic32(&b,0,0,0)==a);
        swr_v1_locks_atomic32(&b,1,v,0);a=v;
        int64_t exp=i%3?(int64_t)next():c;
        CHECK(swr_v1_locks_atomic64(&d,2,(int64_t)v,0)==__sync_add_and_fetch(&c,(int64_t)v));
        CHECK(swr_v1_locks_atomic64(&d,3,(int64_t)(v>>1),0)==__sync_sub_and_fetch(&c,(int64_t)(v>>1)));
        CHECK(swr_v1_locks_atomic64(&d,4,v,exp)==__sync_bool_compare_and_swap(&c,exp,(int64_t)v));
        CHECK(swr_v1_locks_atomic64(&d,0,0,0)==c);
        swr_v1_locks_atomic64(&d,1,v,0);c=v;
    }
    alignas(8) int64_t sum=0;std::vector<std::thread> workers;
    for(int i=0;i<4;i++)workers.emplace_back([&]{for(int j=0;j<20000;j++)swr_v1_locks_atomic64(&sum,2,1,0);});
    for(auto&t:workers)t.join();CHECK(sum==80000);
    CHECK(swr_v1_locks_atomic32(nullptr,2,1,0)==0);
}

static void coroutine_policy() {
    uint32_t word=0;
    CHECK(swr_v1_locks_coroutine_try(&word,0,0,42,7,1)==1);CHECK(word==1);
    CHECK(swr_v1_locks_coroutine_try(&word,42,7,42,7,1)==0);CHECK(word==1);
    CHECK(swr_v1_locks_coroutine_try(&word,42,7,42,8,0)==2);CHECK(word==1);
    CHECK(swr_v1_locks_coroutine_try(&word,42,7,43,7,1)==3);CHECK(word==1);
    CHECK(swr_v1_locks_coroutine_release(&word)==1);CHECK(word==0);
    CHECK(swr_v1_locks_coroutine_release(&word)==0);
    // Pinned owner reentrancy precedes CAS even if the shared word is already zero.
    CHECK(swr_v1_locks_coroutine_try(&word,42,7,42,7,0)==0);CHECK(word==0);
    CHECK(swr_v1_locks_coroutine_try(&word,42,7,42,8,0)==1);CHECK(word==1);
    double second=0.001;
    for(double expected: {0.002,0.004,0.008,0.016,0.032,0.064,0.1,0.001}) {
        second=swr_v1_locks_coroutine_backoff(second,0.001,0.1);CHECK(second==expected);
    }
}

static void wait_wake() {
    // Rust implements only the futex path; non-futex builds run upstream's registry.
#ifdef HAVE_FUTEX
#define WAIT(w,t) (rust?swr_v1_locks_wait(w,t):entry::wait(w,t))
#define WAKE(w,n) (rust?swr_v1_locks_wakeup(w,n):entry::wake(w,n))
    for(int rust: {0,1}) {
#else
#define WAIT(w,t) entry::wait(w,t)
#define WAKE(w,n) entry::wake(w,n)
    entry::wait_init();
    {
#endif
        auto *word=(uint32_t*)mmap(nullptr,4096,PROT_READ|PROT_WRITE,MAP_SHARED|MAP_ANON,-1,0);CHECK(word!=MAP_FAILED);
        *word=1;
        CHECK(WAIT(word,0.01)==0);CHECK(*word==0);
        CHECK(WAIT(word,0.002)==-1);CHECK(*word==0);
        pid_t pid=fork();CHECK(pid>=0);
        if(!pid){int r=WAIT(word,1);_exit(r==0?0:21);}
        usleep(10000);int r=WAKE(word,1);CHECK(r>=0);
        int status;CHECK(waitpid(pid,&status,0)==pid);CHECK(WIFEXITED(status)&&WEXITSTATUS(status)==0);munmap(word,4096);
    }
#undef WAIT
#undef WAKE
}

int main() {
    CHECK(swr_v1_locks_size(3)==entry::size(3));
    CHECK(swr_v1_locks_align(3)==entry::align(3));
    CHECK(swr_v1_locks_size(1)==entry::size(1));
    CHECK(swr_v1_locks_align(1)==entry::align(1));
    CHECK((entry::exercise()==exercise<RustLock<3>,RustLock<1>,swoole::Barrier>()));
#ifdef HAVE_SPINLOCK
    auto reference_spin=entry::spin(),rust_spin=spin<RustLock<5>>();
    for(size_t i=0;i<reference_spin.size();i++)CHECK(reference_spin[i]==rust_spin[i]);
#endif
    CHECK(swr_v1_locks_init(nullptr,3,0)==EINVAL);CHECK(swr_v1_locks_size(99)==0);
    shared_barrier<swoole::Barrier>();
#ifndef SW_USE_PTHREAD_BARRIER
    entry::shared_barrier();
#endif
    atomics();wait_wake();coroutine_policy();std::cout<<"{\"checks\":"<<checks<<",\"passed\":true}"<<std::endl;
}
#endif
