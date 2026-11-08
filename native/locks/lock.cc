// SPDX-License-Identifier: Apache-2.0
// swoole_lock.h's Mutex and Barrier, which the server and core-tests construct, over the
// Rust lock ABI (swoole_rs_locks.h). A throwing constructor is what stable Rust can't
// express.
#include "swoole_lock.h"
#include "swoole_rs_locks.h"
#include <cstddef>
#include <system_error>

namespace swoole {
Mutex::Mutex(bool shared) : shared_(shared) {
    const size_t size = swr_v1_locks_size(Lock::MUTEX);
    if (!size || swr_v1_locks_align(Lock::MUTEX) > alignof(std::max_align_t)) {
        throw std::system_error(EINVAL, std::generic_category(), "unsupported Rust lock storage");
    }
    impl = reinterpret_cast<MutexImpl *>(shared ? sw_mem_pool()->alloc(size) : ::operator new(size));
    if (!impl) { throw std::bad_alloc(); }
    if (swr_v1_locks_init((void *) impl, Lock::MUTEX, shared) != 0) {
        const int error = errno;
        if (shared) { sw_mem_pool()->free((void *) impl); } else { ::operator delete((void *) impl); }
        throw std::system_error(error, std::generic_category(), "pthread_mutex_init() failed");
    }
}
int Mutex::lock(int operation, int timeout_msec) {
    uint32_t capabilities = 0;
#ifdef HAVE_MUTEX_TIMEDLOCK
    capabilities |= 1;
#endif
#ifdef HAVE_RWLOCK_TIMEDRDLOCK
    capabilities |= 2;
#endif
    return swr_v1_locks_lock((void *) impl, Lock::MUTEX, operation, timeout_msec, capabilities);
}
int Mutex::unlock() { return swr_v1_locks_unlock((void *) impl, Lock::MUTEX); }
Mutex::~Mutex() {
    swr_v1_locks_destroy((void *) impl, Lock::MUTEX);
    if (shared_) { sw_mem_pool()->free((void *) impl); } else { ::operator delete((void *) impl); }
}

void Barrier::init(bool shared, int count) {
#ifdef SW_USE_PTHREAD_BARRIER
    shared_ = shared;
    swr_v1_locks_pthread_barrier(&barrier_, sizeof(barrier_), &barrier_attr_, sizeof(barrier_attr_), 0, shared_, count);
#else
    swr_v1_locks_barrier_init((uint32_t *) &count_, (uint32_t *) &barrier_, count);
#endif
}
void Barrier::wait() {
#ifdef SW_USE_PTHREAD_BARRIER
    swr_v1_locks_pthread_barrier(&barrier_, sizeof(barrier_), &barrier_attr_, sizeof(barrier_attr_), 1, shared_, 0);
#else
    swr_v1_locks_barrier_wait((uint32_t *) &count_, (uint32_t *) &barrier_);
#endif
}
void Barrier::destroy() {
#ifdef SW_USE_PTHREAD_BARRIER
    swr_v1_locks_pthread_barrier(&barrier_, sizeof(barrier_), &barrier_attr_, sizeof(barrier_attr_), 2, shared_, 0);
#endif
}
}  // namespace swoole
