/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_lock.h, owned here: Mutex (core-tests construct it) and the
 * server's Barrier, over Rust's lock ABI (lock.cc). Swoole\Lock's RW, spin and coroutine
 * locks call that ABI directly and have no C++ class. */
#pragma once

#include "swoole_memory.h"

#include <sys/file.h>

namespace swoole {

// The lock kinds of Rust's lock ABI (swoole_rs_locks.h), as Swoole\Lock numbers them.
struct Lock {
    enum Type {
        RW_LOCK = 1,
        MUTEX = 3,
        SPIN_LOCK = 5,
        COROUTINE_LOCK = 6,
    };
};

struct MutexImpl;

class Mutex {
    MutexImpl *impl;
    bool shared_;

  public:
    explicit Mutex(bool shared);
    Mutex(const Mutex &) = delete;
    Mutex &operator=(const Mutex &) = delete;
    ~Mutex();
    int lock(int operation = LOCK_EX, int timeout_msec = -1);
    int unlock();
};

#if defined(HAVE_PTHREAD_BARRIER) && !(defined(__FreeBSD__) || defined(__OpenBSD__) || defined(__NetBSD__))
#define SW_USE_PTHREAD_BARRIER
#endif

struct Barrier {
#ifdef SW_USE_PTHREAD_BARRIER
    pthread_barrier_t barrier_;
    pthread_barrierattr_t barrier_attr_;
    bool shared_;
#else
    sw_atomic_t count_;
    sw_atomic_t barrier_;
#endif
    void init(bool shared, int count);
    void wait();
    void destroy();
};

}  // namespace swoole
