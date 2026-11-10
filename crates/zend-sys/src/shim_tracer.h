/* Shims the tracer uses (crates/phpcoroutines, swoole_tracer.cc): the request
 * heap's custom handlers, whose signatures depend on the PHP version and debug
 * mode, and the executor, output, number-format and JSON calls bindgen cannot
 * reach. */
#ifndef SWOOLE_RS_ZEND_SHIM_TRACER_H
#define SWOOLE_RS_ZEND_SHIM_TRACER_H
#include "php.h"
#include "zend_observer.h"
#include "zend_builtin_functions.h"

/* What the tracer sees of the request heap. `alloc` and `realloc` run with
 * the observer removed (they may allocate), `free` with it in place. */
typedef struct swr_zend_mm_observer {
    void (*alloc)(void *ptr, size_t size);
    void (*realloc)(void *ptr, void *new_ptr, size_t size);
    void (*free)(void *ptr);
} swr_zend_mm_observer;

/* hook_emalloc: keep the heap's current custom handlers and install ones that
 * call them (or the heap itself), then `observer`. */
void swr_zend_mm_observe(const swr_zend_mm_observer *observer);
/* unhook_emalloc: restore the kept handlers, or turn custom handlers off. */
void swr_zend_mm_unobserve(void);

/* The line of the innermost user frame (the line before an exception being
 * handled), or 0. */
uint32_t swr_zend_tracer_lineno(void);

/* ZEND_WRITE(text, length) inside zend_try (output handlers run PHP): 1 if it
 * bailed out. */
int swr_zend_try_write(const char *text, size_t length);

/* number_format(number) with "." and ",": a request string. */
zend_string *swr_zend_number_format(zend_long number);

/* php_json_encode(value, 0): a request string, or NULL on failure. */
zend_string *swr_zend_json_encode(zval *value);

/* The same inside zend_try (a JsonSerializable runs PHP): 1 if it bailed out;
 * *out is the string, or NULL on failure or bailout. */
int swr_zend_try_json_encode(zval *value, zend_string **out);

/* json_decode(text, true) into `out` (depth 0, as Server::command passes), or false. */
void swr_zend_json_decode(const char *text, size_t length, zval *out);

/* Swoole's zend::json_decode inside zend_try: json_decode()'s empty-string and
 * depth checks, then php_json_decode_ex into `out`. 1 if it bailed out. */
int swr_zend_try_json_decode_ex(zval *out, const char *text, size_t length, zend_long options, zend_long depth);
#endif
