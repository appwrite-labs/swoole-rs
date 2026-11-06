#ifndef SWOOLE_RS_PHPSERVER_SETTINGS_H
#define SWOOLE_RS_PHPSERVER_SETTINGS_H
/* Swoole\Server::set() (crates/phpserver, settings.rs). Rust reads every
 * option in upstream's order, converts and bounds it and writes the Server's
 * fields; C++ answers the build and process facts below (`get`) and keeps the
 * effects that need its C++ API. The ids are Rust's. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_object;
struct _zval_struct;

enum {
    SWR_PHPSERVER_SET_CPU_NUM = 100,
    SWR_PHPSERVER_SET_GLOBAL_ENABLE_COROUTINE = 101,
    SWR_PHPSERVER_SET_WORKER_THREAD = 102,
    SWR_PHPSERVER_SET_HAVE_REUSE_PORT = 110,
    SWR_PHPSERVER_SET_HAVE_CPU_AFFINITY = 111,
    SWR_PHPSERVER_SET_HAVE_COMPRESSION = 112,
    SWR_PHPSERVER_SET_HAVE_ZLIB = 113,
    SWR_PHPSERVER_SET_HAVE_MSGQUEUE = 114,
    SWR_PHPSERVER_SET_THREAD = 115,
    SWR_PHPSERVER_SET_PATH_MAX = 116,
};

enum {
    SWR_PHPSERVER_TEXT_CHROOT = 1,
    SWR_PHPSERVER_TEXT_USER = 2,
    SWR_PHPSERVER_TEXT_GROUP = 3,
    SWR_PHPSERVER_TEXT_PID_FILE = 4,
    SWR_PHPSERVER_TEXT_TASK_TMPDIR = 5,
    SWR_PHPSERVER_TEXT_UPLOAD_TMP_DIR = 6,
    SWR_PHPSERVER_TEXT_DOCUMENT_ROOT = 7,
    SWR_PHPSERVER_TEXT_INDEX_FILE = 8,
    SWR_PHPSERVER_TEXT_COMPRESSION_TYPE = 9,
    SWR_PHPSERVER_TEXT_STATIC_LOCATION = 10,
};

typedef struct {
    int64_t (*get)(void *server, uint32_t id);
    /* A string setting, or one item of a list (SWR_PHPSERVER_TEXT_*). */
    void (*text)(void *server, uint32_t id, const char *value, size_t length);
    void (*rewrite)(void *server, const char *pattern, size_t pattern_length, const char *replacement,
                    size_t replacement_length);
    int64_t (*parse_size)(struct _zval_struct *value);
    /* php_swoole_set_global/coroutine/aio_option in zend_try: true if it bailed out. */
    bool (*global_options)(void *settings);
    /* dispatch_func: the callable and its hook, in zend_try: true if it bailed out. */
    bool (*dispatch_func)(void *server, struct _zval_struct *value);
    /* This process's CPU affinity into `cpus`: the count, or -1 with *error = errno. */
    int (*cpu_affinity)(int *cpus, size_t capacity, int *error);
    /* serv->cpu_affinity_available(_num) = a copy of `cpus`. */
    void (*set_cpu_affinity)(void *server, const int *cpus, size_t count);
    const char *(*strerror)(int code);
    int (*create_dir)(const char *path, size_t length);
    /* SW_THREAD's bootstrap and init_arguments, in zend_try: true if it bailed out. */
    bool (*thread_options)(struct _zend_object *object, void *settings);
    /* The primary port's set() and the `setting` merge, in zend_try: true if it bailed out. */
    bool (*finish)(struct _zend_object *object, struct _zval_struct *settings);
} swr_v1_phpserver_settings_host;

void swr_v1_phpserver_settings_init(const swr_v1_phpserver_settings_host *table);
#ifdef __cplusplus
}
#endif
#endif
