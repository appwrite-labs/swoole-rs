/* Zend headers for bindgen, plus the macros and inline functions it cannot see.
 * Each shim is the Zend macro it names, compiled against the same headers, so it
 * follows that PHP's version and build mode (ZTS, debug). */
#ifndef SWOOLE_RS_ZEND_SHIM_H
#define SWOOLE_RS_ZEND_SHIM_H
#include "php.h"
#include "../../../native/corebase/swoole_rs_zend_guard.h"
#include "zend_exceptions.h"
#include "zend_interfaces.h"
#include "ext/standard/url.h"
#include "ext/date/php_date.h"

/* ZEND_TYPE_INIT_CLASS_CONST's bit: PHP 8.2 marks a literal class name with
 * the name bit, which zend_register_functions turns into a zend_string. */
#if PHP_VERSION_ID < 80300
#define _ZEND_TYPE_LITERAL_NAME_BIT _ZEND_TYPE_NAME_BIT
#endif

/* zend_object_alloc: emalloc the object and its property slots and zero the
 * bytes before the embedded zend_object. */
void *swr_zend_object_alloc(size_t size, zend_class_entry *ce);

/* INIT_CLASS_ENTRY_EX, then zend_register_internal_class_ex. */
zend_class_entry *swr_zend_register_class(const char *name, size_t length,
                                          const zend_function_entry *functions,
                                          zend_class_entry *parent);

#include "shim_values.h"

/* emalloc and efree (macros taking file and line in debug builds). */
void *swr_zend_emalloc(size_t size);
void swr_zend_efree(void *ptr);

/* Z_TRY_ADDREF_P. */
void swr_zval_try_addref(zval *value);

/* zend_string_init (persistent), zend_string_release and ZVAL_STR (the zval
 * takes over the reference). */
zend_string *swr_zend_string_persistent(const char *text, size_t length);
void swr_zend_string_release(zend_string *string);
void swr_zval_str(zval *value, zend_string *string);

/* Bailouts. zend_try runs `body(arg)`; returns 1 if it bailed out (Zend's
 * longjmp), which is then caught here. `body` must be C or C++: a bailout must
 * never longjmp over a Rust frame. Rust reports the bailout to its C/C++ caller,
 * which re-raises it with zend_bailout() once the Rust frames have returned. */
int swr_zend_try(void (*body)(void *), void *arg);
/* zend_call_function(fci, fcc) inside zend_try: *result is 1 on SUCCESS. */
int swr_zend_try_call(zend_fcall_info *fci, zend_fcall_info_cache *fcc, int *result);

/* Z_PARAM_FUNC of argument `number` (not nullable): 1, or 0 after raising
 * Zend's own parameter error. */
int swr_zend_parse_func(zval *arg, uint32_t number, zend_fcall_info *fci, zend_fcall_info_cache *fcc);
#endif
