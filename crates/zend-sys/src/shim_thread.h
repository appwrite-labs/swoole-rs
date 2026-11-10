/* Shims the thread-shared containers (crates/phpthread) need: inline functions
 * and macros of zend_hash.h, zend_operators.h and zend_execute.h. */
#ifndef SWOOLE_RS_ZEND_SHIM_THREAD_H
#define SWOOLE_RS_ZEND_SHIM_THREAD_H

#include "shim.h"

/* zend_array_is_list. */
bool swr_zend_array_is_list(const zend_array *array);

/* ZEND_HASH_FOREACH_KEY_VAL with ZVAL_DEREF: `key` is null for an int key. */
void swr_zend_hash_foreach_kv(const HashTable *table,
                              void (*each)(zend_string *key, zend_ulong index, zval *value, void *context),
                              void *context);

/* SW_HASHTABLE_FOREACH_START2's step, without ZVAL_DEREF: the element at or after
 * `*position` (null past the end), with its string key in `*key` (null for an int
 * key, which goes in `*index`); `*position` moves past it. Start at 0. */
zval *swr_zend_hash_next(const HashTable *table, HashPosition *position, zend_string **key, zend_ulong *index);

/* _zend_handle_numeric_str: whether a string key is a canonical integer. */
bool swr_zend_handle_numeric_str(const char *key, size_t length, zend_ulong *index);

/* zend_dval_to_lval_safe. */
zend_long swr_zend_dval_to_lval_safe(double value);

/* Z_RES_HANDLE_P. */
zend_long swr_zend_resource_handle(const zval *value);

/* is_numeric_string(text, length, &lval, &dval, false): IS_LONG, IS_DOUBLE or 0. */
uint8_t swr_zend_is_numeric_string(const char *text, size_t length, zend_long *lval, double *dval);
/* zendi_smart_strcmp. */
int swr_zend_smart_strcmp(zend_string *a, zend_string *b);

/* Swoole\Thread's streams. php_stream_open_wrapper_ex(path, mode, 0, NULL, NULL). */
php_stream *swr_zend_stream_open(const char *path, const char *mode);
/* php_stream_cast(stream, PHP_STREAM_AS_FD_FOR_SELECT | PHP_STREAM_CAST_INTERNAL, &fd, 1)
 * inside zend_try: 0 on success, 1 on failure, 2 if a warning's handler bailed out. */
int swr_zend_stream_select_fd(php_stream *stream, int *fd);
/* php_stream_cast(stream, PHP_STREAM_AS_FD, &fd, 0) == SUCCESS. */
bool swr_zend_stream_fd(php_stream *stream, int *fd);
/* Register `stream` as the case-sensitive constant `name` (STDIN and friends). */
void swr_zend_register_stream_constant(const char *name, php_stream *stream);

/* A PHP thread's request (ZTS; elsewhere these do nothing). `enter` takes this
 * thread's TSRM resources (ts_resource(0)) and sets the INI a thread's request
 * starts with: expose_php and enable_dl off, auto_globals_jit on. */
void swr_zend_thread_enter(void);
/* php_request_startup with the main request's script path and argc and no
 * headers; then, inside zend_first_try with *bailout holding its jump buffer,
 * prologue(context) and `file` as the primary script; then, *bailout cleared,
 * epilogue(context) and request shutdown. EG(exit_status), 1 if startup failed. */
int swr_zend_thread_request(const char *file, const char *path_translated, int argc, void (*prologue)(void *),
                            void (*epilogue)(void *), void *context, void **bailout);
/* ts_free_thread(). */
void swr_zend_thread_leave(void);
/* $argv and $argc in the global symbol table, which takes `argv`. */
void swr_zend_thread_set_argv(zval *argv, zend_long argc);
/* The main request's script (SG(request_info).path_translated), argc and global
 * $argv (null if none). */
zval *swr_zend_thread_main_request(const char **path, int *argc);
/* tsrm_is_main_thread(), tsrm_is_shutdown() and tsrm_api_name(). */
bool swr_zend_tsrm_is_main_thread(void);
bool swr_zend_tsrm_is_shutdown(void);
const char *swr_zend_tsrm_api_name(void);
/* EG(bailout) = target unless null, then zend_bailout(). */
void swr_zend_bailout_to(void *target);
#endif
