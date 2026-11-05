#ifndef SWOOLE_RS_PHPHELPERS_H
#define SWOOLE_RS_PHPHELPERS_H
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Complete ownership operations for the source-compatible Zend facades.
 * Handles and zvals are owned by Rust; pointers borrowed from them expire at release. */
struct _zval_struct;
struct _zend_string;
typedef struct SwrPhpString SwrPhpString;
SwrPhpString *swr_v1_phphelpers_string_new(const char *, size_t);
SwrPhpString *swr_v1_phphelpers_string_value(struct _zval_struct *);
SwrPhpString *swr_v1_phphelpers_string_take(struct _zend_string *, bool);
struct _zend_string *swr_v1_phphelpers_string_get(const SwrPhpString *);
void swr_v1_phphelpers_string_free(SwrPhpString *);
void swr_v1_phphelpers_string_trim(const SwrPhpString *);
char *swr_v1_phphelpers_string_dup(const SwrPhpString *, bool);
char *swr_v1_phphelpers_bytes_dup(const char *, size_t, bool, bool);
void swr_v1_phphelpers_bytes_free(char **);
struct _zval_struct *swr_v1_phphelpers_variable_new(void);
void swr_v1_phphelpers_variable_assign(struct _zval_struct *, const struct _zval_struct *);
void swr_v1_phphelpers_variable_free(struct _zval_struct *);
void swr_v1_phphelpers_variable_reset(struct _zval_struct *);
void swr_v1_phphelpers_variable_string(struct _zval_struct *, const char *, size_t);
void swr_v1_phphelpers_variable_move(struct _zval_struct *, struct _zval_struct *);
void swr_v1_phphelpers_variable_ref(struct _zval_struct *, bool);
struct _Bucket;
typedef struct SwrPhpKeyValue SwrPhpKeyValue;
typedef struct {
    struct _zval_struct *value;
    struct _zend_string *key;
    struct _Bucket *current;
    struct _Bucket *end;
    uint64_t index;
} SwrPhpIterator;
SwrPhpKeyValue *swr_v1_phphelpers_key_new(uint64_t, struct _zend_string *, struct _zval_struct *);
void swr_v1_phphelpers_key_add(SwrPhpKeyValue *, struct _zval_struct *);
void swr_v1_phphelpers_key_free(SwrPhpKeyValue *);
struct _zval_struct *swr_v1_phphelpers_key_value(SwrPhpKeyValue *);
SwrPhpIterator swr_v1_phphelpers_iterator_new(struct _Bucket *, struct _Bucket *, bool);
void swr_v1_phphelpers_iterator_next(SwrPhpIterator *);
size_t swr_v1_phphelpers_array_count(const struct _zval_struct *);
bool swr_v1_phphelpers_array_set(struct _zval_struct *, uint64_t, struct _zval_struct *, bool);
struct _zend_fcall_info_cache;
struct _zend_class_entry;
typedef struct SwrPhpCallable SwrPhpCallable;
SwrPhpCallable *swr_v1_phphelpers_callable_new(struct _zval_struct *, void (*)(const char *));
SwrPhpCallable *swr_v1_phphelpers_callable_factory(struct _zval_struct *, const char *, bool, struct _zend_class_entry *, void (*)(const char *));
void swr_v1_phphelpers_callable_free(SwrPhpCallable *);
struct _zend_fcall_info_cache *swr_v1_phphelpers_callable_cache(SwrPhpCallable *);
bool swr_v1_phphelpers_callable_ready(const SwrPhpCallable *);
SwrPhpCallable *swr_v1_phphelpers_callable_copy(const SwrPhpCallable *);
bool swr_v1_phphelpers_callable_invoke(SwrPhpCallable *, uint32_t, struct _zval_struct *, struct _zval_struct *);
uint32_t swr_v1_phphelpers_callable_refs(const SwrPhpCallable *);
/* Native Zend storage/effects remain authoritative. Borrowed initialized spans
 * are used only during each call. No callback or allocation occurs in Rust. */
/* Common Native async admission: initialized phase0; blocking skips context.
 * Native observes context only on request, then invokes fn/async after return.
 * No callback/context representation or frame crosses these Native effects. */
enum SwrPhpAsyncAction { SWR_PHP_ASYNC_DONE=0, SWR_PHP_ASYNC_OBSERVE_COROUTINE=1,
    SWR_PHP_ASYNC_DIRECT=2, SWR_PHP_ASYNC_DISPATCH=3 };
uint32_t swr_v1_phphelpers_async(uint32_t *phase, uint8_t blocking, uint8_t coroutine);
/* Read-only known bytes retain frozen SW_ZEND_KNOWN_STRINGS enum order.
 * Native global persistent/interned pointer storage remains authoritative.
 * Cursor/length are independent initialized native size_t storage. */
size_t swr_v1_phphelpers_known_count(void);
const char *swr_v1_phphelpers_known_next(size_t *cursor, size_t *length);
enum SwrPhpKeyedMergeAction { SWR_PHP_KEYED_DONE=0,
    SWR_PHP_KEYED_ADD_INDEX=1, SWR_PHP_KEYED_ADD_STRING=2,
    SWR_PHP_KEYED_FIND_INDEX=3, SWR_PHP_KEYED_FIND_STRING=4,
    SWR_PHP_KEYED_APPEND=5, SWR_PHP_KEYED_PROMOTE_INDEX=6,
    SWR_PHP_KEYED_PROMOTE_STRING=7 };
/* Same lifecycle/observations as frozen merge; native numeric-key classification
 * precedes phase0. Rust selects API routes at the existing dispatch points. */
uint32_t swr_v1_phphelpers_merge_keyed(uint32_t *, uint8_t inserted,
                                    uint8_t existing_array, uint8_t numeric_key);
uint8_t swr_v1_phphelpers_call_success(int64_t result, uint8_t coroutine);
enum SwrPhpCallAction { SWR_PHP_CALL_NULL=0, SWR_PHP_CALL_DIRECT=1,
    SWR_PHP_CALL_COROUTINE=2, SWR_PHP_CALL_NAMED=3, SWR_PHP_CALL_RELEASE_NAME=4,
    SWR_PHP_CALL_OBSERVE_EXCEPTION=5, SWR_PHP_CALL_FATAL=6,
    SWR_PHP_CALL_TRUE=7, SWR_PHP_CALL_FALSE=8, SWR_PHP_CALL_VARIABLE=9 };
/* Phase0, modes direct0/coroutine1/named2. Native named-call allocation and local
 * Variable construction precede phase0; result and exception refreshed only after
 * invoke/observe actions. Native cleanup and fatal execution occur after return. */
uint32_t swr_v1_phphelpers_call_plan(uint32_t *, uint32_t mode, uint8_t has_retval,
                                   int64_t result, uint8_t exception);

#ifdef SW_STDEXT
/* stdext (crates/phpstdext): typed arrays and methods on scalars, linked only
 * when config.h enables SW_STDEXT. Rust exports the swoole_* function handlers
 * (zif_*) under Zend's own names. */
struct _zval_struct;
struct _zend_array;
struct _zend_string;
struct _zend_execute_data;
/* The PHP library calls C++ lends at MINIT. */
typedef struct {
    void (*json_decode)(struct _zval_struct *value, const char *text, size_t length, int64_t options, int64_t depth);
    /* php_pcre_match_impl of a cached compiled regex (RETVAL_FALSE if it does
     * not compile); flags count as passed when num_args >= 3. */
    void (*pcre_match)(struct _zend_string *regex, struct _zend_string *subject, struct _zval_struct *value,
                       uint8_t global, uint32_t num_args, int64_t flags, int64_t offset);
} SwrPhpStdextOps;
void swr_v1_phpstdext_minit(const SwrPhpStdextOps *ops);
/* User opcode handlers: INIT_METHOD_CALL, UNSET_DIM and FE_RESET_RW. */
int swr_v1_phpstdext_method_call(struct _zend_execute_data *frame);
int swr_v1_phpstdext_unset(struct _zend_execute_data *frame);
int swr_v1_phpstdext_foreach_begin(struct _zend_execute_data *frame);
/* Whether key => value may go in the typed array; if not, Zend has thrown. */
bool swr_v1_phpstdext_check(struct _zend_array *ht, const struct _zval_struct *key, const struct _zval_struct *value);
/* After SEPARATE_ARRAY copied the typed array source, give the copy its type. */
void swr_v1_phpstdext_copy_type(struct _zval_struct *container, struct _zend_array *source);
/* ASSIGN_DIM_OP on a typed array: concat needs strings, arithmetic numbers;
 * if not, Zend has thrown. */
bool swr_v1_phpstdext_operation(struct _zend_array *ht, bool concat);
#endif
#ifdef __cplusplus
}
#endif
#endif
