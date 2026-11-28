#ifndef SWOOLE_RS_SERVER_STRINGS_H
#define SWOOLE_RS_SERVER_STRINGS_H
#include "swoole_rs_server_state.h"
#include <iterator>
#include <memory>
#include <string>
#include <unordered_set>
#include <vector>
namespace swoole { namespace detail {
// Source-compatible collection view. All retained bytes and insertion/search
// policy belong to Rust; copied strings below are temporary native call inputs.
class ServerStrings {
    swr_v1_server_state *state_;
    uint32_t kind_;
    size_t operation(uint32_t op, const char *data = nullptr, size_t length = 0) const {
        return swr_v1_server_static_config(state_, kind_, op, reinterpret_cast<const uint8_t *>(data), length);
    }
  public:
    struct iterator {
        using iterator_category = std::input_iterator_tag;
        using value_type = std::string;
        using difference_type = ptrdiff_t;
        using pointer = void;
        using reference = std::string;
        const ServerStrings *owner;
        size_t index;
        std::string operator*() const { return (*owner)[index]; }
        iterator &operator++() { ++index; return *this; }
        iterator operator++(int) { auto copy=*this; ++*this; return copy; }
        bool operator==(const iterator &other) const { return owner==other.owner && index==other.index; }
        bool operator!=(const iterator &other) const { return !(*this==other); }
    };
    ServerStrings(swr_v1_server_state *state, uint32_t kind): state_(state), kind_(kind) {}
    ServerStrings(const ServerStrings &) = delete;
    ServerStrings &operator=(const ServerStrings &) = delete;
    ServerStrings *operator->() { return this; }
    const ServerStrings *operator->() const { return this; }
    ServerStrings &operator*() { return *this; }
    const ServerStrings &operator*() const { return *this; }
    explicit operator bool() const { return operation(1); }
    bool operator==(std::nullptr_t) const { return !bool(*this); }
    bool operator!=(std::nullptr_t) const { return bool(*this); }
    size_t size() const { return operation(0); }
    bool empty() const { return size()==0; }
    void enable() { operation(2); }
    void insert(const std::string &value) { operation(4,value.data(),value.size()); }
    void emplace(const std::string &value) { insert(value); }
    void emplace_back(const std::string &value) { operation(6,value.data(),value.size()); }
    iterator begin() const { return {this,0}; }
    iterator end() const { return {this,size()}; }
    iterator find(const std::string &value) const { return {this,operation(5,value.data(),value.size())}; }
    std::string operator[](size_t index) const {
        size_t length=0;
        auto *data=swr_v1_server_static_text(state_,kind_,index,&length);
        return data ? std::string(reinterpret_cast<const char *>(data),length) : std::string();
    }
    template<typename Collection> ServerStrings &operator=(const std::shared_ptr<Collection> &values) {
        operation(3);
        if (values) { enable(); for (const auto &value:*values) operation(6,value.data(),value.size()); }
        return *this;
    }
    operator std::vector<std::string>() const { return {begin(),end()}; }
    const void *share() const { return swr_v1_server_static_owner(state_,kind_); }
};
// Existing native coroutine-server sets retain their original shared_ptr owner;
// Server MIME sets share the Rust owner. Neither path snapshots retained bytes.
class ServerMimeTypes {
    const void *rust_ = nullptr;
    std::shared_ptr<std::unordered_set<std::string>> native_;
    void clear() { swr_v1_server_static_owner_free(rust_); rust_ = nullptr; native_.reset(); }
  public:
    ServerMimeTypes() = default;
    ServerMimeTypes(std::nullptr_t) {}
    ServerMimeTypes(const ServerMimeTypes &other): rust_(other.rust_ ? swr_v1_server_static_owner_clone(other.rust_) : nullptr), native_(other.native_) {}
    ~ServerMimeTypes() { swr_v1_server_static_owner_free(rust_); }
    ServerMimeTypes &operator=(const ServerMimeTypes &other) {
        if (this != &other) { clear(); rust_=other.rust_ ? swr_v1_server_static_owner_clone(other.rust_) : nullptr; native_=other.native_; }
        return *this;
    }
    ServerMimeTypes &operator=(std::nullptr_t) { clear(); return *this; }
    ServerMimeTypes &operator=(const ServerStrings &values) { clear(); rust_=bool(values) ? values.share() : nullptr; return *this; }
    ServerMimeTypes &operator=(const std::shared_ptr<std::unordered_set<std::string>> &values) { clear(); native_=values; return *this; }
    ServerMimeTypes *operator->() { return this; }
    const ServerMimeTypes *operator->() const { return this; }
    explicit operator bool() const { return rust_ ? swr_v1_server_static_mime(rust_,nullptr,0) : bool(native_); }
    bool find(const std::string &value) const {
        return rust_ ? swr_v1_server_static_mime(rust_,reinterpret_cast<const uint8_t *>(value.data()),value.size())
                     : native_ && native_->find(value)!=native_->end();
    }
    bool end() const { return false; }
};
}}
#endif
