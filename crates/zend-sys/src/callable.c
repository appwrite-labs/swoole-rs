/* Callables kept past a call. Stays C (docs/zend.md, What stays C): allocator
 * (zval_ptr_dtor, efree, OBJ_RELEASE), private (ZEND_CLOSURE_OBJECT reaches into
 * zend_closures.c's struct), and swr_zend_zval_release is a body for zend_try. */
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
