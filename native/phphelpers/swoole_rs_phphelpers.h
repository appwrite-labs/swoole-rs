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
struct _zval_struct *swr_v1_phphelpers_variable_new(void);
void swr_v1_phphelpers_variable_assign(struct _zval_struct *, const struct _zval_struct *);
void swr_v1_phphelpers_variable_free(struct _zval_struct *);
void swr_v1_phphelpers_variable_reset(struct _zval_struct *);
void swr_v1_phphelpers_variable_string(struct _zval_struct *, const char *, size_t);
void swr_v1_phphelpers_variable_move(struct _zval_struct *, struct _zval_struct *);
void swr_v1_phphelpers_variable_ref(struct _zval_struct *, bool);
struct _Bucket;
typedef struct {
    struct _zval_struct *value;
    struct _zend_string *key;
    struct _Bucket *current;
    struct _Bucket *end;
    uint64_t index;
} SwrPhpIterator;
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
struct _zend_object;
struct _zend_object_handlers;
struct _zend_fcall_info;
struct _zval_struct *swr_v1_phphelpers_zval_alloc(void);
struct _zval_struct *swr_v1_phphelpers_zval_dup(const struct _zval_struct *);
struct _zval_struct *swr_v1_phphelpers_property(struct _zend_class_entry *, struct _zval_struct *, const char *, size_t, bool, bool);
struct _zval_struct *swr_v1_phphelpers_property_offset(struct _zend_class_entry *, struct _zval_struct *, struct _zend_string *);
void swr_v1_phphelpers_cache_free(struct _zend_fcall_info_cache *);
int swr_v1_phphelpers_call(struct _zval_struct *, struct _zend_fcall_info_cache *, uint32_t, struct _zval_struct *, struct _zval_struct *, bool, void (*)(const char *));
int swr_v1_phphelpers_call_anyway(struct _zend_fcall_info *, struct _zend_fcall_info_cache *);
/* 1 called, 0 failed, -1 bailed out (the caller re-raises). The leaf is swr_v1_phpcoroutines_create. */
int swr_v1_phphelpers_function_call(struct _zend_fcall_info_cache *, uint32_t, struct _zval_struct *, struct _zval_struct *, bool,
    long (*)(struct _zend_fcall_info_cache *, uint32_t, struct _zval_struct *, struct _zval_struct *, uint8_t *),
    void (*)(const char *));
void swr_v1_phphelpers_named_call(const char *, size_t, uint32_t, struct _zval_struct *, struct _zval_struct *);
union _zend_function;
struct _zend_array;
void swr_v1_phphelpers_array_merge(struct _zval_struct *, const char *, size_t, struct _zval_struct *);
void swr_v1_phphelpers_known_init(struct _zend_string ***);
void swr_v1_phphelpers_known_free(struct _zend_string ***);
union _zend_function *swr_v1_phphelpers_function_find(const struct _zend_array *, const char *, size_t);
/* The remaining async admission seam borrows native facts only. */
/* Common Native async admission: initialized phase0; blocking skips context.
 * Native observes context only on request, then invokes fn/async after return.
 * No callback/context representation or frame crosses these Native effects. */
enum SwrPhpAsyncAction { SWR_PHP_ASYNC_DONE=0, SWR_PHP_ASYNC_OBSERVE_COROUTINE=1,
    SWR_PHP_ASYNC_DIRECT=2, SWR_PHP_ASYNC_DISPATCH=3 };
uint32_t swr_v1_phphelpers_async(uint32_t *phase, uint8_t blocking, uint8_t coroutine);

#ifdef SW_STDEXT
/* stdext (crates/phpstdext): typed arrays and methods on scalars, linked only
 * when config.h enables SW_STDEXT. Rust exports the swoole_* function handlers
 * (zif_*) under Zend's own names. */
struct _zval_struct;
struct _zend_array;
struct _zend_string;
struct _zend_execute_data;
void swr_v1_phpstdext_minit(void);
int swr_v1_phpstdext_assign(struct _zend_execute_data *frame);
int swr_v1_phpstdext_assign_op(struct _zend_execute_data *frame);
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
struct _zend_execute_data;
bool swr_v1_phphelpers_callable_frame(struct _zval_struct *value, struct _zval_struct *object,
    struct _zend_execute_data *frame, uint32_t flags, char **name, size_t *length,
    struct _zend_fcall_info_cache *cache, char **error);
bool swr_v1_phphelpers_unset_class_valid(struct _zend_object *object);
void swr_v1_phphelpers_unset_deny(struct _zend_object *object, struct _zend_string *member, void **cache);
int swr_v1_phphelpers_class_alias(const char *name, size_t length, struct _zend_class_entry *entry);
#ifdef __cplusplus
}
#endif
#endif
