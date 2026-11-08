/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_version.h, owned here: the macros C++ and the compiled core-tests
 * read. Rust defines swoole_version() and swoole_version_id(). */
#pragma once

#define SWOOLE_VERSION "6.2.5"
#define SWOOLE_VERSION_ID 60205
#define SWOOLE_API_VERSION_ID 0x202208a

#define SWOOLE_BUG_REPORT                                                                                              \
    "A process crash occurred in Swoole-v" SWOOLE_VERSION ". Please report this issue.\n"                              \
    "You can refer to the documentation below, submit an issue to us on GitHub.\n"                                     \
    ">> https://github.com/swoole/swoole-src/blob/master/docs/ISSUE.md\n"
