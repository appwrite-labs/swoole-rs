/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include <cstdlib>

namespace swoole_rs {
// Optional native PHP transport. Registration happens during module startup,
// before requests or worker publication; the provider has static lifetime and
// is never replaced afterward. Standalone C++ SDK consumers keep a null slot.
struct NativeEffectProvider {
    int (*invoke)(void (*body)(void *), void *argument);
    void (*resume)();
};
namespace detail {
inline const NativeEffectProvider *&native_effect_slot() {
    static const NativeEffectProvider *provider = nullptr;
    return provider;
}
}  // namespace detail
inline const NativeEffectProvider *native_effect_provider() {
    return detail::native_effect_slot();
}
inline void register_native_effect_provider(const NativeEffectProvider *provider) {
    auto &slot = detail::native_effect_slot();
    if (!provider || (slot && slot != provider)) std::abort();
    slot = provider;
}
}  // namespace swoole_rs
