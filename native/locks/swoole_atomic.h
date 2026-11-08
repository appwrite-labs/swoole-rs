/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_atomic.h, owned here: the atomic word types and the four
 * operations C++ still uses. swoole.h includes it from the Rust prefix's include path;
 * the spin lock and futex waits are Rust's (crates/utilities, crates/phplocks). */
#pragma once

typedef volatile int64_t sw_atomic_long_t;
typedef volatile uint32_t sw_atomic_t;

#define sw_atomic_cmp_set(lock, old, set) __sync_bool_compare_and_swap(lock, old, set)
#define sw_atomic_fetch_add(value, add) __sync_fetch_and_add(value, add)
#define sw_atomic_fetch_sub(value, sub) __sync_fetch_and_sub(value, sub)
#define sw_atomic_add_fetch(value, add) __sync_add_and_fetch(value, add)
