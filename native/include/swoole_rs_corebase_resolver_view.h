/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include "swoole_rs_corebase_resolver.h"
#include <new>
#include <cstdlib>
#include <stdexcept>
#include <vector>
#include <string>
namespace swoole {
class ResolverOwner {
  protected:
    swr_v1_core_resolver_prepared state_;
    ResolverOwner(int family, int socktype, int protocol)
        : state_(swr_v1_core_resolver_new(family, socktype, protocol)) {
        if (!state_.owner) throw std::bad_alloc();
    }
    ResolverOwner(int family, int socktype, int protocol, const std::string &name, const std::string &service)
        : state_(swr_v1_core_resolver_create(family, socktype, protocol, name.data(), name.size(), service.data(), service.size())) {
        if (!state_.owner) throw std::bad_alloc();
    }
    ~ResolverOwner() { swr_v1_core_resolver_free(state_.owner); }
    ResolverOwner(const ResolverOwner &) = delete;
    ResolverOwner &operator=(const ResolverOwner &) = delete;
  public:
    static void check(int status) {
        if (status == -2) throw std::bad_alloc();
        if (status == -4) throw std::length_error("DNS address vector length");
        if (status < 0) std::abort();
    }
};
// Mutable borrowed spans always name the actual Rust vector. Copying a view
// creates an independent byte snapshot; it never copies ownership of a borrow.
class ResolverAddresses {
    swr_v1_core_resolver *owner_;
    bool owned_;
    friend struct GetaddrinfoRequest;
    explicit ResolverAddresses(swr_v1_core_resolver *owner) : owner_(owner), owned_(false) {}
  public:
    using value_type = sockaddr_in6;
    using size_type = size_t;
    using iterator = sockaddr_in6 *;
    using const_iterator = const sockaddr_in6 *;
    ResolverAddresses() : owner_(swr_v1_core_resolver_new(AF_INET, 0, 0).owner), owned_(true) {
        if (!owner_) throw std::bad_alloc();
    }
    ResolverAddresses(const ResolverAddresses &source) : ResolverAddresses() {
        *this = source;
    }
    ~ResolverAddresses() { if (owned_) swr_v1_core_resolver_free(owner_); }
    ResolverAddresses &operator=(const ResolverAddresses &source) {
        if (this != &source) assign(source.data(), source.size());
        return *this;
    }
    ResolverAddresses &operator=(const std::vector<sockaddr_in6> &source) {
        assign(source.data(), source.size()); return *this;
    }
    void assign(const sockaddr_in6 *source, size_t count) {
        ResolverOwner::check(swr_v1_core_resolver_assign(owner_, source, count));
    }
    template<class Iterator> void assign(Iterator first, Iterator last) {
        std::vector<sockaddr_in6> snapshot(first, last); *this = snapshot;
    }
    size_t size() const { return swr_v1_core_resolver_span(owner_).length; }
    size_t capacity() const { return swr_v1_core_resolver_span(owner_).capacity; }
    bool empty() const { return size() == 0; }
    sockaddr_in6 *data() { return swr_v1_core_resolver_span(owner_).data; }
    const sockaddr_in6 *data() const { return swr_v1_core_resolver_span(owner_).data; }
    iterator begin() { return data(); }
    const_iterator begin() const { return data(); }
    iterator end() { auto span = swr_v1_core_resolver_span(owner_); return span.length ? span.data + span.length : span.data; }
    const_iterator end() const { auto span = swr_v1_core_resolver_span(owner_); return span.length ? span.data + span.length : span.data; }
    sockaddr_in6 &operator[](size_t index) { return data()[index]; }
    const sockaddr_in6 &operator[](size_t index) const { return data()[index]; }
    sockaddr_in6 &at(size_t index) { if (index >= size()) throw std::out_of_range("DNS address"); return (*this)[index]; }
    const sockaddr_in6 &at(size_t index) const { if (index >= size()) throw std::out_of_range("DNS address"); return (*this)[index]; }
    sockaddr_in6 &front() { return (*this)[0]; }
    const sockaddr_in6 &front() const { return (*this)[0]; }
    sockaddr_in6 &back() { return (*this)[size() - 1]; }
    const sockaddr_in6 &back() const { return (*this)[size() - 1]; }
    void resize(size_t count) { ResolverOwner::check(swr_v1_core_resolver_resize(owner_, count)); }
    void clear() { resize(0); }
    void push_back(const sockaddr_in6 &value) {
        const auto copy = value;
        const size_t count = size();
        resize(count + 1); (*this)[count] = copy;
    }
    operator std::vector<sockaddr_in6>() const {
        return empty() ? std::vector<sockaddr_in6>{} : std::vector<sockaddr_in6>{begin(), end()};
    }
};
} // namespace swoole
