#include "shim_host.h"

int swr_zend_register_global_alias(const char *origin, size_t origin_length, const char *alias,
                                   size_t alias_length, const zend_internal_arg_info *arg_info) {
    zend_string *lowercase_origin = zend_string_alloc(origin_length, 0);
    zend_str_tolower_copy(ZSTR_VAL(lowercase_origin), origin, origin_length);
    zend_function *origin_function = zend_hash_find_ptr(CG(function_table), lowercase_origin);
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
