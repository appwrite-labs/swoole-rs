#ifndef SWOOLE_RS_PHPHELPERS_H
#define SWOOLE_RS_PHPHELPERS_H
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Native Zend storage/effects remain authoritative. Borrowed initialized spans
 * are used only during each call. No callback or allocation occurs in Rust. */
/* Common Native async admission: initialized phase0; blocking skips context.
 * Native observes context only on request, then invokes fn/async after return.
 * No callback/context representation or frame crosses these Native effects. */
enum SwrPhpAsyncAction { SWR_PHP_ASYNC_DONE=0, SWR_PHP_ASYNC_OBSERVE_COROUTINE=1,
    SWR_PHP_ASYNC_DIRECT=2, SWR_PHP_ASYNC_DISPATCH=3 };
uint32_t swr_v1_phphelpers_async(uint32_t *phase, uint8_t blocking, uint8_t coroutine);
/* Native parsing/string-length/array-count observation occurs before this call. */
uint8_t swr_v1_phphelpers_empty(size_t count);
/* Public native String/CharPtr lifecycle; initialized phase0, copied observations.
 * op0 release;1 copy assignment;2 move assignment;3 release/copy;4 retain/take.
 * Observe means native presence for0, self identity for1; no other reads.
 * Native executes every action only after Rust returns, then refreshes facts. */
enum SwrPhpStringAction { SWR_PHP_STRING_DONE=0, SWR_PHP_STRING_OBSERVE=1,
    SWR_PHP_STRING_RELEASE=2, SWR_PHP_STRING_RESET=3, SWR_PHP_STRING_COPY=4,
    SWR_PHP_STRING_MOVE=5, SWR_PHP_STRING_RESET_SOURCE=6, SWR_PHP_STRING_ALIAS=7 };
uint32_t swr_v1_phphelpers_string_plan(uint32_t *phase, uint32_t operation, uint8_t observation);
/* Native KeyValue ctor0/insert1/dtor2; Variable retained-copy3/move4.
 * Phase0; key presence observed freshly on request. Native storage/effects only. */
enum SwrPhpValueAction { SWR_PHP_FACADE_DONE=0, SWR_PHP_FACADE_OBSERVE_KEY=1,
    SWR_PHP_FACADE_RETAIN_KEY=2, SWR_PHP_FACADE_NULL_KEY=3,
    SWR_PHP_FACADE_DEREFERENCE=4, SWR_PHP_FACADE_COPY=5, SWR_PHP_FACADE_RETAIN=6,
    SWR_PHP_FACADE_INSERT_STRING=7, SWR_PHP_FACADE_INSERT_INDEX=8,
    SWR_PHP_FACADE_RETAIN_INSERTED=9, SWR_PHP_FACADE_RELEASE_KEY=10,
    SWR_PHP_FACADE_DESTROY=11, SWR_PHP_FACADE_RESET_SOURCE=12 };
uint32_t swr_v1_phphelpers_value_plan(uint32_t *phase, uint32_t operation, uint8_t key_present);
/* Inline Callable duplicate: native allocation precedes phase0. */
enum SwrPhpDuplicateAction { SWR_PHP_DUPLICATE_DONE=0, SWR_PHP_DUPLICATE_CACHE=1,
    SWR_PHP_DUPLICATE_VALUE=2, SWR_PHP_DUPLICATE_RETAIN=3,
    SWR_PHP_DUPLICATE_OBSERVE_NAME=4, SWR_PHP_DUPLICATE_NAME=5 };
uint32_t swr_v1_phphelpers_duplicate_plan(uint32_t *phase, uint8_t name_present);
/* Native callable factory0 / nullable factory1, phase0; observations lazy. */
enum SwrPhpFactoryAction { SWR_PHP_FACTORY_NULL=0, SWR_PHP_FACTORY_OBSERVE_READY=1,
    SWR_PHP_FACTORY_DELETE=2, SWR_PHP_FACTORY_RETURN=3,
    SWR_PHP_FACTORY_OBSERVE_POINTER=4, SWR_PHP_FACTORY_OBSERVE_NULL=5,
    SWR_PHP_FACTORY_CREATE=6, SWR_PHP_FACTORY_OBSERVE_CREATED=7,
    SWR_PHP_FACTORY_NULL_ERROR=8, SWR_PHP_FACTORY_INVALID_ERROR=9 };
uint32_t swr_v1_phphelpers_factory_plan(uint32_t *phase, uint32_t operation,
                                      uint8_t observation, uint8_t allow_null);
/* Whole sparse/indirect iterator policy; native bounds/storage/API decoding. */
enum SwrPhpIteratorAction { SWR_PHP_ITERATOR_DONE=0, SWR_PHP_ITERATOR_OBSERVE_END=1,
    SWR_PHP_ITERATOR_READ_VALUE=2, SWR_PHP_ITERATOR_OBSERVE_POINTER=3,
    SWR_PHP_ITERATOR_OBSERVE_INDIRECT=4, SWR_PHP_ITERATOR_DEREFERENCE=5,
    SWR_PHP_ITERATOR_OBSERVE_UNDEFINED=6, SWR_PHP_ITERATOR_ADVANCE=7,
    SWR_PHP_ITERATOR_OBSERVE_KEY=8, SWR_PHP_ITERATOR_PUBLISH_STRING=9,
    SWR_PHP_ITERATOR_PUBLISH_INDEX=10 };
uint32_t swr_v1_phphelpers_iterator_plan(uint32_t *phase, uint8_t observation);
/* Read-only known bytes retain frozen SW_ZEND_KNOWN_STRINGS enum order.
 * Native global persistent/interned pointer storage remains authoritative.
 * Cursor/length are independent initialized native size_t storage. */
size_t swr_v1_phphelpers_known_count(void);
const char *swr_v1_phphelpers_known_next(size_t *cursor, size_t *length);
enum SwrPhpReleaseAction { SWR_PHP_RELEASE_DONE=0, SWR_PHP_RELEASE_OBSERVE_VALUE=1,
    SWR_PHP_RELEASE_VALUE=2, SWR_PHP_RELEASE_OBSERVE_NAME=3, SWR_PHP_RELEASE_NAME=4 };
/* Phase0, freshly observed value-defined then name-present AFTER native dtor. */
uint32_t swr_v1_phphelpers_release(uint32_t *, uint8_t observation);
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
enum SwrPhpCallableAction { SWR_PHP_CALLABLE_DONE=0, SWR_PHP_CALLABLE_TRUTH=1,
    SWR_PHP_CALLABLE_CHECK=2, SWR_PHP_CALLABLE_ILLEGAL=3,
    SWR_PHP_CALLABLE_INVALID=4, SWR_PHP_CALLABLE_RETAIN=5 };
uint32_t swr_v1_phphelpers_callable(uint32_t *, uint8_t truthy, uint8_t valid);

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
