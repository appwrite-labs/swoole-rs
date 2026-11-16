/* Stays C (docs/zend.md, What stays C): globals (EG and CG function tables,
 * EG(uninitialized_zval), the VM stack zend_vm_stack_push_call_frame grows),
 * bailout (calls, autoloading, array writes). */
#include "shim.h"
#include "shim_stdext.h"
#include "SAPI.h"
#include "php_variables.h"

/* crates/phpstdext's opcode handlers return these by value. */
_Static_assert(ZEND_USER_OPCODE_CONTINUE == 0, "ZEND_USER_OPCODE_CONTINUE");
_Static_assert(ZEND_USER_OPCODE_DISPATCH == 2, "ZEND_USER_OPCODE_DISPATCH");

zend_function *swr_zend_compiled_function_find(const char *name) {
    return zend_hash_str_find_ptr(CG(function_table), name, strlen(name));
}

zif_handler swr_zend_minit_replace_handler(const char *name, zif_handler handler) {
    zend_function *fn = swr_zend_compiled_function_find(name);
    if (!fn || fn->type != ZEND_INTERNAL_FUNCTION) {
        return NULL;
    }
    zif_handler original = fn->internal_function.handler;
    fn->internal_function.handler = handler;
    return original;
}

int swr_zend_try_call_known(zend_function *fn, zval *retval, swr_zvals args) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        zend_call_known_function(fn, NULL, NULL, retval, args.argc, args.argv, NULL);
    }
    SWR_ZEND_CATCH {
        bailed = 1;
    }
    SWR_ZEND_END_TRY();
    return bailed;
}

int swr_zend_try_lookup_class(zend_string *name, zend_class_entry **ce) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        *ce = zend_lookup_class(name);
    }
    SWR_ZEND_CATCH {
        bailed = 1;
    }
    SWR_ZEND_END_TRY();
    return bailed;
}

void swr_zend_push_nested_call(zend_execute_data *execute_data, zend_function *fn, uint32_t num_args,
                               void *object, void *run_time_cache) {
    zend_execute_data *call = zend_vm_stack_push_call_frame(ZEND_CALL_NESTED_FUNCTION, fn, num_args, object);
    call->run_time_cache = run_time_cache;
    call->prev_execute_data = EX(call);
    EX(call) = call;
}

void swr_zend_treat_string(char *text, zval *array) {
    sapi_module.treat_data(PARSE_STRING, text, array);
}

int swr_zend_try_stdext_nogc_dtor(zval *value) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { zval_ptr_dtor_nogc(value); } SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
