#ifndef SWOOLE_RS_COREBASE_WORKER_REGISTRY_VIEW_H
#define SWOOLE_RS_COREBASE_WORKER_REGISTRY_VIEW_H
#include "swoole_rs_corebase_worker_registry.h"
#ifdef __cplusplus
#include <new>
namespace swoole { namespace detail {
// Collection ends before native effects. No registry borrow or snapshot-creation
// frame spans signalling, wait callbacks or native vector allocation.
// A borrowed configuration view; the existing Rust registry owns the ordered
// list. Its allocation and lifetime are shared with the server configuration.
template<typename Worker> class PendingWorkers {
    swr_v1_process_worker_registry *registry_;
  public:
    struct iterator {
        void *const *position;
        Worker *operator*() const { return static_cast<Worker *>(*position); }
        iterator &operator++() { ++position; return *this; }
        bool operator!=(const iterator &other) const { return position != other.position; }
    };
    explicit PendingWorkers(void *registry): registry_(static_cast<swr_v1_process_worker_registry *>(registry)) {}
    size_t size() const { return swr_v1_process_worker_registry_pending_count(registry_); }
    bool empty() const { return size()==0; }
    Worker *operator[](size_t index) const { return static_cast<Worker *>(data()[index]); }
    void push_back(Worker *worker) { swr_v1_process_worker_registry_pending_add(registry_,worker); }
    void *const *data() const { return swr_v1_process_worker_registry_pending_data(registry_); }
    iterator begin() const { return {data()}; }
    iterator end() const { auto *p=data(); auto n=size(); return {n ? p+n : p}; }
};
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
