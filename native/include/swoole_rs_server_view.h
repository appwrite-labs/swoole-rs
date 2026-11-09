#ifndef SWOOLE_RS_SERVER_VIEW_H
#define SWOOLE_RS_SERVER_VIEW_H
#include "swoole_rs_server_state.h"
#include <type_traits>
#include <utility>

namespace swoole { namespace detail {
// A view of one Rust-owned callback slot. Assigning a callable boxes it, and the slot's
// `drop` frees the box: building and destroying a C++ lambda is the one part Rust can't
// do. Rust may store its own `invoke` with a null `ctx`.
template <class Sig> class Callback;
template <class R, class... A> class Callback<R(A...)> : swr_v1_server_callback {
    template <class T> static R call(void *boxed, A... args) {
        return static_cast<R>((*static_cast<T *>(boxed))(std::forward<A>(args)...));
    }
    template <class T> static bool engaged(const T &f, std::true_type) {
        return static_cast<bool>(f);
    }
    template <class T> static bool engaged(const T &, std::false_type) {
        return true;
    }
    template <class T> static void destroy(void *boxed) noexcept {
        delete static_cast<T *>(boxed);
    }
    void clear() {
        auto *old_drop = drop;
        auto *old_ctx = ctx;
        invoke = nullptr;
        ctx = nullptr;
        drop = nullptr;
        if (old_drop) old_drop(old_ctx);
    }

  public:
    Callback() = delete;
    Callback(const Callback &) = delete;
    Callback &operator=(const Callback &) = delete;
    Callback &operator=(std::nullptr_t) {
        clear();
        return *this;
    }
    // A null function pointer or an empty std::function empties the slot, as it
    // empties a std::function.
    template <class F> Callback &operator=(F &&f) {
        using T = typename std::decay<F>::type;
        T *boxed = engaged<T>(f, std::is_constructible<bool, const T &>{}) ? new T(std::forward<F>(f)) : nullptr;
        clear();
        if (boxed) {
            ctx = boxed;
            drop = &destroy<T>;
            invoke = reinterpret_cast<void (*)(void)>(&call<T>);
        }
        return *this;
    }
    explicit operator bool() const {
        return invoke != nullptr;
    }
    R operator()(A... args) const {
        return reinterpret_cast<R (*)(void *, A...)>(invoke)(ctx, std::forward<A>(args)...);
    }
    friend bool operator==(const Callback &c, std::nullptr_t) {
        return !c;
    }
    friend bool operator!=(const Callback &c, std::nullptr_t) {
        return bool(c);
    }
};

} }
#endif
