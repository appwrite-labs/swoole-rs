#ifndef SWOOLE_RS_COREBASE_WORKER_REGISTRY_VIEW_H
#define SWOOLE_RS_COREBASE_WORKER_REGISTRY_VIEW_H
#include "swoole_rs_corebase_worker_registry.h"
#ifdef __cplusplus
#include <new>
namespace swoole { namespace detail {
// Collection ends before native effects. No registry borrow or snapshot-creation
// frame spans signalling, wait callbacks or native vector allocation.
template<typename Worker> class RegisteredWorkers {
    swr_v1_worker_snapshot snapshot_{};
  public:
    struct iterator {
        void *const *position;
        Worker *operator*() const { return static_cast<Worker *>(*position); }
        iterator &operator++() { ++position; return *this; }
        bool operator!=(const iterator &other) const { return position != other.position; }
    };
    RegisteredWorkers(const swr_v1_process_worker_registry *registry, int category) {
        if (!swr_v1_process_worker_registry_snapshot(registry, category, &snapshot_)) throw std::bad_alloc();
    }
    RegisteredWorkers(const RegisteredWorkers &) = delete;
    RegisteredWorkers &operator=(const RegisteredWorkers &) = delete;
    ~RegisteredWorkers() { swr_v1_process_worker_registry_snapshot_free(&snapshot_); }
    iterator begin() const { return {snapshot_.data}; }
    iterator end() const { return {snapshot_.length ? snapshot_.data + snapshot_.length : snapshot_.data}; }
};
}}
#endif
#endif
