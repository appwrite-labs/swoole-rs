#include "shim_stdext.h"
#include "SAPI.h"
#include "php_variables.h"

/* crates/phpstdext's opcode handlers return these by value. */
_Static_assert(ZEND_USER_OPCODE_CONTINUE == 0, "ZEND_USER_OPCODE_CONTINUE");
_Static_assert(ZEND_USER_OPCODE_DISPATCH == 2, "ZEND_USER_OPCODE_DISPATCH");

zend_function *swr_zend_function_find(const char *name, size_t length) {
    return zend_hash_str_find_ptr(EG(function_table), name, length);
}

zend_function *swr_zend_compiled_function_find(const char *name, size_t length) {
    return zend_hash_str_find_ptr(CG(function_table), name, length);
}

int swr_zend_try_call_known(zend_function *fn, zval *retval, uint32_t argc, zval *argv) {
    int bailed = 0;
    zend_try {
        zend_call_known_function(fn, NULL, NULL, retval, argc, argv, NULL);
    }
    zend_catch {
        bailed = 1;
    }
    zend_end_try();
    return bailed;
}

int swr_zend_try_lookup_class(zend_string *name, zend_class_entry **ce) {
    int bailed = 0;
    zend_try {
        *ce = zend_lookup_class(name);
    }
    zend_catch {
        bailed = 1;
    }
    zend_end_try();
    return bailed;
}

zend_execute_data *swr_zend_push_nested_call(zend_function *fn, uint32_t num_args, void *object) {
    return zend_vm_stack_push_call_frame(ZEND_CALL_NESTED_FUNCTION, fn, num_args, object);
}

zval *swr_zend_uninitialized_zval(void) {
    return &EG(uninitialized_zval);
}

void swr_zend_treat_string(char *text, zval *array) {
    sapi_module.treat_data(PARSE_STRING, text, array);
}

void swr_zend_hash_foreach_key(const HashTable *table,
                               bool (*each)(zend_ulong index, zend_string *key, zval *value, void *context),
                               void *context) {
    zend_ulong index;
    zend_string *key;
    zval *value;
    ZEND_HASH_FOREACH_KEY_VAL((HashTable *) table, index, key, value) {
        if (!each(index, key, value, context)) {
            break;
        }
    }
    ZEND_HASH_FOREACH_END();
}
