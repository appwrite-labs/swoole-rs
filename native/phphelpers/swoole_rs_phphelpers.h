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
struct _zend_fcall_info_cache;
struct _zend_class_entry;
struct _zend_object;
struct _zend_object_handlers;
struct _zend_fcall_info;
struct _zval_struct *swr_v1_phphelpers_zval_dup(const struct _zval_struct *);
struct _zval_struct *swr_v1_phphelpers_property(struct _zend_class_entry *, struct _zval_struct *, const char *, size_t, bool, bool);
void swr_v1_phphelpers_cache_free(struct _zend_fcall_info_cache *);
int swr_v1_phphelpers_call(struct _zval_struct *, struct _zend_fcall_info_cache *, uint32_t, struct _zval_struct *, struct _zval_struct *, bool, void (*)(const char *));
int swr_v1_phphelpers_call_anyway(struct _zend_fcall_info *, struct _zend_fcall_info_cache *);
void swr_v1_phphelpers_named_call(const char *, size_t, uint32_t, struct _zval_struct *, struct _zval_struct *);
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
int swr_v1_phphelpers_class_alias(const char *name, size_t length, struct _zend_class_entry *entry);
#ifdef __cplusplus
}
#endif
#endif
