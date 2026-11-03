#ifndef SWOOLE_RS_PHPSERVER_SETTINGS_H
#define SWOOLE_RS_PHPSERVER_SETTINGS_H
/* Swoole\Server::set() (crates/phpserver, settings.rs). Rust reads every
 * option in upstream's order and converts and bounds it; C++ keeps the Server's
 * fields, set and read by id through `host`. The ids below are Rust's. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_object;
struct _zval_struct;

enum {
    SWR_PHPSERVER_SET_DAEMONIZE = 1,
    SWR_PHPSERVER_SET_REACTOR_NUM = 2,
    SWR_PHPSERVER_SET_SINGLE_THREAD = 3,
    SWR_PHPSERVER_SET_WORKER_NUM = 4,
    SWR_PHPSERVER_SET_MAX_WAIT_TIME = 5,
    SWR_PHPSERVER_SET_MAX_QUEUED_BYTES = 6,
    SWR_PHPSERVER_SET_MAX_CONCURRENCY = 7,
    SWR_PHPSERVER_SET_WORKER_MAX_CONCURRENCY = 8,
    SWR_PHPSERVER_SET_ENABLE_COROUTINE = 9,
    SWR_PHPSERVER_SET_DISPATCH_MODE = 10,
    SWR_PHPSERVER_SET_SEND_YIELD = 11,
    SWR_PHPSERVER_SET_DISCARD_TIMEOUT_REQUEST = 12,
    SWR_PHPSERVER_SET_ENABLE_UNSAFE_EVENT = 13,
    SWR_PHPSERVER_SET_ENABLE_DELAY_RECEIVE = 14,
    SWR_PHPSERVER_SET_ENABLE_REUSE_PORT = 15,
    SWR_PHPSERVER_SET_TASK_OBJECT = 16,
    SWR_PHPSERVER_SET_EVENT_OBJECT = 17,
    SWR_PHPSERVER_SET_TASK_ENABLE_COROUTINE = 18,
    SWR_PHPSERVER_SET_TASK_WORKER_NUM = 19,
    SWR_PHPSERVER_SET_TASK_IPC_MODE = 20,
    SWR_PHPSERVER_SET_TASK_MAX_REQUEST = 21,
    SWR_PHPSERVER_SET_TASK_MAX_REQUEST_GRACE = 22,
    SWR_PHPSERVER_SET_MAX_CONNECTION = 23,
    SWR_PHPSERVER_SET_START_SESSION_ID = 24,
    SWR_PHPSERVER_SET_HEARTBEAT_CHECK_INTERVAL = 25,
    SWR_PHPSERVER_SET_MAX_REQUEST = 26,
    SWR_PHPSERVER_SET_MAX_REQUEST_GRACE = 27,
    SWR_PHPSERVER_SET_RELOAD_ASYNC = 28,
    SWR_PHPSERVER_SET_OPEN_CPU_AFFINITY = 29,
    SWR_PHPSERVER_SET_HTTP_PARSE_COOKIE = 30,
    SWR_PHPSERVER_SET_HTTP_PARSE_POST = 31,
    SWR_PHPSERVER_SET_HTTP_PARSE_FILES = 32,
    SWR_PHPSERVER_SET_HTTP_COMPRESSION = 33,
    SWR_PHPSERVER_SET_HTTP_COMPRESSION_LEVEL = 34,
    SWR_PHPSERVER_SET_COMPRESSION_MIN_LENGTH = 35,
    SWR_PHPSERVER_SET_WEBSOCKET_COMPRESSION = 36,
    SWR_PHPSERVER_SET_UPLOAD_MAX_FILESIZE = 37,
    SWR_PHPSERVER_SET_ENABLE_STATIC_HANDLER = 38,
    SWR_PHPSERVER_SET_HTTP_AUTOINDEX = 39,
    SWR_PHPSERVER_SET_INPUT_BUFFER_SIZE = 40,
    SWR_PHPSERVER_SET_OUTPUT_BUFFER_SIZE = 41,
    SWR_PHPSERVER_SET_MESSAGE_QUEUE_KEY = 42,
    SWR_PHPSERVER_SET_CPU_NUM = 100,
    SWR_PHPSERVER_SET_GLOBAL_ENABLE_COROUTINE = 101,
    SWR_PHPSERVER_SET_WORKER_THREAD = 102,
    SWR_PHPSERVER_SET_STARTED = 103,
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
    void (*set)(void *server, uint32_t id, int64_t value);
    void (*set_double)(void *server, double send_timeout);
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
    bool (*msgqueue_key_valid)(int64_t key);
    /* SW_THREAD's bootstrap and init_arguments, in zend_try: true if it bailed out. */
    bool (*thread_options)(struct _zend_object *object, void *settings);
    /* The primary port's set() and the `setting` merge, in zend_try: true if it bailed out. */
    bool (*finish)(struct _zend_object *object, struct _zval_struct *settings);
    void (*set_last_error)(int code);
    bool (*display_errors)(void);
} swr_v1_phpserver_settings_host;

void swr_v1_phpserver_settings_init(const swr_v1_phpserver_settings_host *table);
#ifdef __cplusplus
}
#endif
#endif
