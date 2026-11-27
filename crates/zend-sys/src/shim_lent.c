/* Stays C (docs/zend.md, What stays C): bailout (property hooks, methods run PHP)
 * and globals (the executor's uninitialized property sentinel), allocator (Zend strings). */
#include "shim_lent.h"

/* allocator, bailout: allocate the persistent string and trap the intern handler. */
int swr_lent_intern(struct iovec bytes, zend_string **out) {
    zend_string *string = zend_string_init(bytes.iov_base, bytes.iov_len, 1);
    volatile int bailed = 0;
    SWR_ZEND_TRY { *out = zend_new_interned_string(string); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

/* bailout: the class registration and interning steps can escape. */
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

/* globals: compare the declared property's slot with EG(uninitialized_zval). */
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

/* bailout: a property read can invoke a user hook. */
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

/* bailout: a write can invoke a hook or destroy the previous value. */
int swr_lent_update_property(zend_class_entry *scope, zend_object *object, struct iovec name, zval *value) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { zend_update_property(scope, object, name.iov_base, name.iov_len, value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

/* bailout: the called PHP method may exit. */
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

/* globals, bailout: run a cleanup method with the original exception suspended. */
int swr_lent_call_cleanup(zend_object *object, struct iovec name) {
    zend_object *exception = EG(exception);
    EG(exception) = NULL;
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        zend_call_method(object, NULL, NULL, name.iov_base, name.iov_len, NULL, 0, NULL, NULL);
        if (exception && EG(exception)) {
            zend_clear_exception();
        }
    }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    if (exception) {
        EG(exception) = exception;
    }
    return bailed;
}

void swr_lent_push_bytes(zval *array, struct iovec bytes) {
    /* allocator: ZVAL_STRINGL creates the Zend-owned element. */
    zval string;
    ZVAL_STRINGL(&string, bytes.iov_base, bytes.iov_len);
    zend_hash_next_index_insert(Z_ARRVAL_P(array), &string);
}
