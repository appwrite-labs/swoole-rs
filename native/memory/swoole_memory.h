/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_memory.h, owned here: the global pool over Rust's pages
 * (crates/memory/src/members.rs) and the sw_shm_* calls C++ still makes. Rust makes
 * SwooleG.memory_pool (swoole_rs_memory::members::pool); core-tests make their own
 * and call its members, and its constructor throws. Its one member is Rust's impl. */
#pragma once

#include "swoole.h"
#include "swoole_rs_memory.h"

#include <new>

namespace swoole {

static inline void memory_check(int status) {
    if (status == -1) swoole_throw_error(SW_ERROR_INVALID_PARAMS);
    if (status) throw std::bad_alloc();
}

struct GlobalMemoryImpl;

// Pages allocated once for the program's life; free is a no-op.
class GlobalMemory {
    GlobalMemoryImpl *impl;

  public:
    GlobalMemory(uint32_t page_size, bool shared) {
        memory_check(swr_v1_memory_global_memory_new(page_size, shared, (void **) &impl));
    }
    ~GlobalMemory() {
        swr_v1_memory_global_memory_delete(impl);
    }
    void *alloc(uint32_t size) {
        void *ptr;
        memory_check(swr_v1_memory_global_memory_alloc((void **) &impl, size, &ptr));
        return ptr;
    }
    void free(void *) {}
    size_t get_memory_size() const;
};
}  // namespace swoole

void *sw_shm_malloc(size_t size);
void sw_shm_free(void *ptr);
int sw_shm_protect(void *ptr, int flags);
