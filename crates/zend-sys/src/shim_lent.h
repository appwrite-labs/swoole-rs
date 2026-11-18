/* Functions over typed Zend handles and a byte range Rust lends as a `struct iovec`
 * (std's `IoSlice`, ABI-compatible with one), so crates/zend declares them over its
 * own types, safely. The `swr_lent_` prefix keeps them out of bindgen's allowlist.
 * Each reads the range's `iov_len` bytes only. */
#ifndef SWOOLE_RS_ZEND_SHIM_LENT_H
#define SWOOLE_RS_ZEND_SHIM_LENT_H

/* shim.h (php.h) first, as shim_bytes.h explains. */
#include "shim.h"
#include <sys/uio.h>

/* zend_new_interned_string of a new persistent string of the bytes, inside zend_try
 * (nonzero for a bailout): `*out` then holds the interned string. */
int swr_lent_intern(struct iovec bytes, zend_string **out);

/* swr_lent_intern of `alias`, then zend_register_class_alias_ex(alias, ce, true), inside
 * zend_try (nonzero for a bailout); `*registered` says whether Zend registered it. */
int swr_lent_class_alias(struct iovec alias, zend_class_entry *ce, bool *registered);

/* sw_zend_read_property_ex without its copy: null, or the slot of the instance
 * property `object`'s class declares as `name` (an uninitialised typed one is set
 * to null first). */
zval *swr_lent_property_slot(zend_object *object, struct iovec name);

/* zend_read_property(scope, object, name, silent, rv) inside zend_try (nonzero for a
 * bailout): `rv` then holds what it read, a copy (ZVAL_COPY) unless Zend computed it
 * there. */
int swr_lent_read_property(zend_class_entry *scope, zend_object *object, struct iovec name, bool silent,
                           zval *rv);

/* zend_update_property(scope, object, name, value) inside zend_try (nonzero for a
 * bailout). */
int swr_lent_update_property(zend_class_entry *scope, zend_object *object, struct iovec name, zval *value);

/* zend_call_method(object, scope, NULL, name, retval, ...) inside zend_try (nonzero
 * for a bailout), with `arg1`, then `arg2`, as far as they are not null. A null scope
 * is the object's class; nothing is called for an object that is no `scope`. */
int swr_lent_call_method(zend_object *object, zend_class_entry *scope, struct iovec name, zval *retval,
                         zval *arg1, zval *arg2);

/* add_next_index_stringl(array, bytes): the array, separated, takes a new string of
 * the bytes under its next index (none at ZEND_LONG_MAX, leaking it, as Zend's). */
void swr_lent_push_bytes(zval *array, struct iovec bytes);

#endif
