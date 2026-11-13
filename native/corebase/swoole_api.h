/* SPDX-License-Identifier: Apache-2.0 */
/* Derived from Swoole v6.2.4 include/swoole_api.h (Swoole contributors): swoole_add_hook,
 * which crates/corebase/src/hook.rs defines and the reactor/base fixture calls with
 * upstream's hook type numbers. */
#pragma once

enum swGlobalHookType : int {};

extern "C" {
void swoole_add_hook(swGlobalHookType type, void (*cb)(void *data), int push_back);
}
