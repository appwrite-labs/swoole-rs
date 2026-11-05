#include "swoole_rs_allocation_definitions.h"
// Keep nothrow allocations in the same malloc/free domain as the fault oracle.
void *operator new(size_t n, const std::nothrow_t &) noexcept {
    try { return ::operator new(n); } catch (...) { return nullptr; }
}
void *operator new[](size_t n, const std::nothrow_t &) noexcept {
    try { return ::operator new[](n); } catch (...) { return nullptr; }
}
void operator delete(void *p, const std::nothrow_t &) noexcept { ::operator delete(p); }
void operator delete[](void *p, const std::nothrow_t &) noexcept { ::operator delete[](p); }
