/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_memory.h, owned here: the global pool over Rust's pages
 * (crates/memory/src/members.rs) and the sw_shm_* calls C++ still makes. The pool stays
 * a C++ object because Rust's Table and core-tests call it through its vtable
 * (destructors, alloc, free); its constructor throws. */
#pragma once

#include "swoole.h"
#include "swoole_rs_memory.h"

#include <new>

namespace swoole {

class MemoryPool {
  public:
    virtual ~MemoryPool() = default;
    virtual void *alloc(uint32_t size) = 0;
    virtual void free(void *ptr) = 0;
};

static inline void memory_check(int status) {
    if (status == -1) swoole_throw_error(SW_ERROR_INVALID_PARAMS);
    if (status) throw std::bad_alloc();
}

struct GlobalMemoryImpl;

// Pages allocated once for the program's life; free is a no-op.
class GlobalMemory : public MemoryPool {
    GlobalMemoryImpl *impl;

  public:
    GlobalMemory(uint32_t page_size, bool shared) {
        memory_check(swr_v1_memory_global_memory_new(page_size, shared, (void **) &impl));
    }
    ~GlobalMemory() override {
        swr_v1_memory_global_memory_delete(impl);
    }
    void *alloc(uint32_t size) override {
        void *ptr;
        memory_check(swr_v1_memory_global_memory_alloc((void **) &impl, size, &ptr));
        return ptr;
    }
    void free(void *) override {}
    size_t get_memory_size() const;
};
}  // namespace swoole

void *sw_shm_malloc(size_t size);
void sw_shm_free(void *ptr);
int sw_shm_protect(void *ptr, int flags);
