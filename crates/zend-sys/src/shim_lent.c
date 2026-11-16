/* Stays C (docs/zend.md, What stays C): bailout (property hooks, methods run PHP)
 * and budget (OBJ_PROP). */
#include "shim_lent.h"

int swr_lent_intern(struct iovec bytes, zend_string **out) {
    zend_string *string = zend_string_init(bytes.iov_base, bytes.iov_len, 1);
    volatile int bailed = 0;
    SWR_ZEND_TRY { *out = zend_new_interned_string(string); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

int swr_lent_class_alias(struct iovec alias, zend_class_entry *ce, bool *registered) {
    zend_string *interned = NULL;
    if (swr_lent_intern(alias, &interned)) {
        return 1;
    }
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        *registered = zend_register_class_alias_ex(ZSTR_VAL(interned), ZSTR_LEN(interned), ce, true) == SUCCESS;
    }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

zval *swr_lent_hash_find(const HashTable *table, struct iovec key) {
    return zend_hash_str_find(table, key.iov_base, key.iov_len);
}

zval *swr_lent_property_slot(zend_object *object, struct iovec name) {
    zend_property_info *info = zend_hash_str_find_ptr(&object->ce->properties_info, name.iov_base, name.iov_len);
    if (UNEXPECTED(info == NULL || (info->flags & ZEND_ACC_STATIC))) {
        return NULL;
    }
    zval *property = OBJ_PROP(object, info->offset);
    if (UNEXPECTED(property == &EG(uninitialized_zval))) {
        ZVAL_NULL(property);
    }
    return property;
}

int swr_lent_read_property(zend_class_entry *scope, zend_object *object, struct iovec name, bool silent,
                           zval *rv) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        zval *found = zend_read_property(scope, object, name.iov_base, name.iov_len, silent, rv);
        if (found != rv) {
            ZVAL_COPY(rv, found);
        }
    }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

int swr_lent_update_property(zend_class_entry *scope, zend_object *object, struct iovec name, zval *value) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { zend_update_property(scope, object, name.iov_base, name.iov_len, value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

int swr_lent_set_property(zend_class_entry *scope, zend_object *object, struct iovec name, zval *value) {
    if (EXPECTED(object->ce->type == ZEND_INTERNAL_CLASS && Z_TYPE_P(value) != IS_OBJECT)) {
        zend_update_property(scope, object, name.iov_base, name.iov_len, value);
        return 0;
    }
    return swr_lent_update_property(scope, object, name, value);
}

int swr_lent_call_method(zend_object *object, zend_class_entry *scope, struct iovec name, zval *retval,
                         zval *arg1, zval *arg2) {
    if (scope && !instanceof_function(object->ce, scope)) {
        return 0;
    }
    uint32_t argc = arg1 ? (arg2 ? 2 : 1) : 0;
    volatile int bailed = 0;
    SWR_ZEND_TRY { zend_call_method(object, scope, NULL, name.iov_base, name.iov_len, retval, argc, arg1, arg2); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

zend_string *swr_lent_format_date(struct iovec format, zend_long timestamp, bool local) {
    return php_format_date(format.iov_base, format.iov_len, timestamp, local);
}

void swr_lent_push_bytes(zval *array, struct iovec bytes) {
    /* add_next_index_stringl's body, so the push stays one call. */
    zval string;
    ZVAL_STRINGL(&string, bytes.iov_base, bytes.iov_len);
    zend_hash_next_index_insert(Z_ARRVAL_P(array), &string);
}
