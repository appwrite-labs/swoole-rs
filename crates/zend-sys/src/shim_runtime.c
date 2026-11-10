/* Swoole\Runtime's function and class table edits (crates/phpruntime). Stays C
 * (docs/zend.md, What stays C): allocator (pemalloc, zend_string_dup), version
 * (ZEND_TYPE_LIST_FOREACH_MUTABLE, ZPP macros, ce->parent in an anonymous union),
 * globals (EG(class_table)) and bailout (a replaced handler runs PHP). */
#include "shim_runtime.h"
#include "shim.h"

int swr_zend_runtime_version(void) {
    return PHP_VERSION_ID;
}

zend_internal_arg_info *swr_zend_runtime_copy_arginfo(const zend_internal_function *function,
                                                      zend_internal_arg_info *arg_info_) {
    uint32_t num_args = function->num_args + 1;
    zend_internal_arg_info *arg_info = arg_info_ - 1;

    zend_internal_arg_info *new_arg_info = pemalloc(sizeof(zend_internal_arg_info) * num_args, 1);
    memcpy(new_arg_info, arg_info, sizeof(zend_internal_arg_info) * num_args);

    if (function->fn_flags & ZEND_ACC_VARIADIC) {
        num_args++;
    }

    for (uint32_t i = 0; i < num_args; i++) {
        if (ZEND_TYPE_HAS_LIST(arg_info[i].type)) {
            zend_type_list *old_list = ZEND_TYPE_LIST(arg_info[i].type);
            zend_type_list *new_list = pemalloc(ZEND_TYPE_LIST_SIZE(old_list->num_types), 1);
            memcpy(new_list, old_list, ZEND_TYPE_LIST_SIZE(old_list->num_types));
            ZEND_TYPE_SET_PTR(new_arg_info[i].type, new_list);

            zend_type *list_type;
#if PHP_VERSION_ID >= 80500
            ZEND_TYPE_LIST_FOREACH_MUTABLE(new_list, list_type) {
#else
            ZEND_TYPE_LIST_FOREACH(new_list, list_type) {
#endif
                if (ZEND_TYPE_HAS_NAME(*list_type)) {
                    zend_string *name = zend_string_dup(ZEND_TYPE_NAME(*list_type), 1);
                    ZEND_TYPE_SET_PTR(*list_type, name);
                }
            }
            ZEND_TYPE_LIST_FOREACH_END();
        } else if (ZEND_TYPE_HAS_NAME(arg_info[i].type)) {
            zend_string *name = zend_string_dup(ZEND_TYPE_NAME(arg_info[i].type), 1);
            ZEND_TYPE_SET_PTR(new_arg_info[i].type, name);
        }
    }

    return new_arg_info + 1;
}

void swr_zend_runtime_free_arginfo(const zend_internal_function *function, zend_internal_arg_info *copy) {
    zend_internal_arg_info *arg_info = copy - 1;
    uint32_t num_args = function->num_args + 1;
    if (function->fn_flags & ZEND_ACC_VARIADIC) {
        num_args++;
    }
    for (uint32_t i = 0; i < num_args; i++) {
        zend_type_release(arg_info[i].type, 1);
    }
    pefree(arg_info, 1);
}

zend_class_entry *swr_zend_runtime_find_class(const char *name, size_t length) {
    zend_string *class_name = zend_string_init(name, length, 0);
    zend_string *lcname = zend_string_tolower(class_name);
    zend_class_entry *ce = zend_hash_find_ptr(EG(class_table), lcname);
    zend_string_release(lcname);
    zend_string_release(class_name);
    return ce;
}

zend_class_entry *swr_zend_runtime_parent(const zend_class_entry *ce) {
    return ce->parent;
}

void swr_zend_runtime_set_parent(zend_class_entry *ce, zend_class_entry *parent) {
    ce->parent = parent;
}

bool swr_zend_runtime_instanceof(const zend_class_entry *child, const zend_class_entry *parent) {
    return instanceof_function(child, parent);
}

typedef struct {
    zif_handler handler;
    zend_execute_data *execute_data;
    zval *return_value;
} swr_zend_runtime_handler_call;

static void swr_zend_runtime_handler_body(void *arg) {
    swr_zend_runtime_handler_call *call = arg;
    call->handler(call->execute_data, call->return_value);
}

int swr_zend_runtime_call_handler(zif_handler handler, zend_execute_data *execute_data, zval *return_value) {
    swr_zend_runtime_handler_call call = {handler, execute_data, return_value};
    return swr_zend_try(swr_zend_runtime_handler_body, &call);
}

void ZEND_FASTCALL swr_zend_runtime_disabled(INTERNAL_FUNCTION_PARAMETERS) {
    (void) execute_data;
    (void) return_value;
    zend_error(E_WARNING, "%s() has been disabled for security reasons", get_active_function_name());
}

void ZEND_FASTCALL swr_zend_runtime_unsupported(INTERNAL_FUNCTION_PARAMETERS) {
    (void) execute_data;
    (void) return_value;
    zend_error(E_WARNING, "%s() is not supported in Swoole coroutines", get_active_function_name());
}

int swr_zend_runtime_select_args(zend_execute_data *execute_data,
                                 zval **read,
                                 zval **write,
                                 zval **except,
                                 zend_long *seconds,
                                 bool *seconds_null,
                                 zend_long *micros,
                                 bool *micros_null) {
    ZEND_PARSE_PARAMETERS_START(4, 5)
    Z_PARAM_ARRAY_EX2(*read, 1, 1, 1)
    Z_PARAM_ARRAY_EX2(*write, 1, 1, 1)
    Z_PARAM_ARRAY_EX2(*except, 1, 1, 1)
    Z_PARAM_LONG_OR_NULL(*seconds, *seconds_null)
    Z_PARAM_OPTIONAL
    Z_PARAM_LONG_OR_NULL(*micros, *micros_null)
    ZEND_PARSE_PARAMETERS_END_EX(return 0);
    return 1;
}

void swr_zend_runtime_env_lock(bool lock) {
#ifdef ZTS
    if (lock) {
        tsrm_env_lock();
    } else {
        tsrm_env_unlock();
    }
#else
    (void) lock;
#endif
}

void *swr_zend_runtime_transport(const char *name, size_t length) {
    return zend_hash_str_find_ptr(php_stream_xport_get_hash(), name, length);
}

int swr_zend_runtime_set_transport(const char *name, void *factory, bool remove) {
    if (remove) {
        return php_stream_xport_unregister(name);
    }
    return php_stream_xport_register(name, (php_stream_transport_factory) factory);
}
