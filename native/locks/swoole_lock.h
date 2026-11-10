/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_lock.h, owned here: the server's Barrier, whose methods are Rust
 * (crates/locks) under their C++ symbols. Every lock kind is Rust's lock ABI
 * (swoole_rs_locks.h), with no C++ class. */
#pragma once

#include "swoole.h"

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

#if defined(HAVE_PTHREAD_BARRIER) && !(defined(__FreeBSD__) || defined(__OpenBSD__) || defined(__NetBSD__))
#define SW_USE_PTHREAD_BARRIER
#endif

// crates/locks/src/lib.rs's Barrier.
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
