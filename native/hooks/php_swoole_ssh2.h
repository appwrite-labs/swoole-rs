/* The pinned thirdparty/php/ssh2/php_ssh2.h includes this header first: the host
 * and the coroutine ssh2 hook its bundled ext/ssh2 sources call, in place of
 * upstream's ext-src/php_swoole_ssh2.h. */
#pragma once
#include "swoole_rs_phphost.h"
#include <libssh2_publickey.h>
#include "swoole_rs_ssh2.h"
