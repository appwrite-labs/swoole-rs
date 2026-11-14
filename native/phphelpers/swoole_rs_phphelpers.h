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
int swr_v1_phphelpers_call(struct _zval_struct *, struct _zend_fcall_info_cache *, uint32_t, struct _zval_struct *, struct _zval_struct *, bool, void (*)(const char *));
int swr_v1_phphelpers_class_alias(const char *name, size_t length, struct _zend_class_entry *entry);
#ifdef __cplusplus
}
#endif
#endif
