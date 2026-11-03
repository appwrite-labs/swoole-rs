#include "callable.h"

void swr_zend_callable_hold(swr_zend_callable *callable) {
    if (callable->fcc.object) {
        GC_ADDREF(callable->fcc.object);
    }
    if (callable->fcc.function_handler->op_array.fn_flags & ZEND_ACC_CLOSURE) {
        GC_ADDREF(ZEND_CLOSURE_OBJECT(callable->fcc.function_handler));
    }
}

/* What OBJ_RELEASE does without freeing it or making it a possible root. */
static bool swr_quiet_release(zend_object *object) {
    return GC_REFCOUNT(object) > 1 && !GC_MAY_LEAK((zend_refcounted *) object);
}

int swr_zend_callable_release_quiet(swr_zend_callable *callable) {
    zend_object *object = callable->fcc.object;
    zend_object *closure = callable->fcc.function_handler->op_array.fn_flags & ZEND_ACC_CLOSURE
                               ? ZEND_CLOSURE_OBJECT(callable->fcc.function_handler)
                               : NULL;
    if (callable->fci.param_count > 0 || (object && object == closure) ||
        (object && !swr_quiet_release(object)) || (closure && !swr_quiet_release(closure))) {
        return 0;
    }
    if (object) {
        GC_DELREF(object);
    }
    if (closure) {
        GC_DELREF(closure);
    }
    return 1;
}

void swr_zend_callable_release(void *arg) {
    swr_zend_callable *callable = arg;
    if (callable->fci.param_count > 0) {
        for (uint32_t i = 0; i < callable->fci.param_count; i++) {
            zval_ptr_dtor(&callable->fci.params[i]);
        }
        efree(callable->fci.params);
    }
    if (callable->fcc.object) {
        OBJ_RELEASE(callable->fcc.object);
    }
    if (callable->fcc.function_handler->op_array.fn_flags & ZEND_ACC_CLOSURE) {
        OBJ_RELEASE(ZEND_CLOSURE_OBJECT(callable->fcc.function_handler));
    }
}

void swr_zend_zval_release(void *value) {
    zval_ptr_dtor(value);
}

int swr_zend_register_function_alias(zend_class_entry *ce, const char *origin, size_t origin_length,
                                     const char *alias, size_t alias_length,
                                     const zend_internal_arg_info *arg_info) {
    zend_string *lowercase_origin = zend_string_alloc(origin_length, 0);
    zend_str_tolower_copy(ZSTR_VAL(lowercase_origin), origin, origin_length);
    zend_function *origin_function = zend_hash_find_ptr(&ce->function_table, lowercase_origin);
    zend_string_release(lowercase_origin);
    if (UNEXPECTED(!origin_function)) {
        return FAILURE;
    }
    char *name = emalloc(alias_length + 1);
    ((char *) memcpy(name, alias, alias_length))[alias_length] = '\0';
    zend_function_entry zfe[] = {
        {name, origin_function->internal_function.handler, arg_info, origin_function->common.num_args, 0},
        ZEND_FE_END};
    int ret = zend_register_functions(NULL, zfe, CG(function_table), origin_function->common.type);
    efree(name);
    return ret;
}
