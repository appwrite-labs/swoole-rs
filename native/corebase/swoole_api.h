/* SPDX-License-Identifier: Apache-2.0 */
/* Derived from Swoole v6.2.4 include/swoole_api.h (Swoole contributors): the global hook
 * table's C API, which crates/corebase/src/hook.rs defines, and the hook types C++ names. */
#pragma once

enum swGlobalHookType {
    SW_GLOBAL_HOOK_ON_REACTOR_CREATE = 5,
    SW_GLOBAL_HOOK_ON_REACTOR_DESTROY = 9,
    SW_GLOBAL_HOOK_AFTER_FORK = 12,
    SW_GLOBAL_HOOK_USER = 24,
};

extern "C" {
void swoole_add_hook(swGlobalHookType type, void (*cb)(void *data), int push_back);
void swoole_call_hook(swGlobalHookType type, void *arg);
bool swoole_isset_hook(swGlobalHookType type);
}
