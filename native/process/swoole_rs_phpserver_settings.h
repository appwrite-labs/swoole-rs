#ifndef SWOOLE_RS_PHPSERVER_SETTINGS_H
#define SWOOLE_RS_PHPSERVER_SETTINGS_H
/* Swoole\Server::set() (crates/phpserver, settings.rs). Rust reads every
 * option in upstream's order, converts and bounds it and writes the Server's
 * fields; C++ answers the build switches below (`get`). The ids are Rust's. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zval_struct;

enum {
    SWR_PHPSERVER_SET_HAVE_REUSE_PORT = 110,
    SWR_PHPSERVER_SET_HAVE_COMPRESSION = 112,
    SWR_PHPSERVER_SET_HAVE_ZLIB = 113,
    SWR_PHPSERVER_SET_HAVE_MSGQUEUE = 114,
    SWR_PHPSERVER_SET_THREAD = 115,
};

typedef struct {
    int64_t (*get)(void *server, uint32_t id);
} swr_v1_phpserver_settings_host;

void swr_v1_phpserver_settings_init(const swr_v1_phpserver_settings_host *table);
#ifdef __cplusplus
}
#endif
#endif
