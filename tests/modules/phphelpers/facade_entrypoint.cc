// Test-only consumer of the actual public native/M26 facade header.
#include "php_swoole_cxx.h"
#include <utility>
#include <vector>
#include <initializer_list>
ZEND_BEGIN_ARG_INFO_EX(arginfo_m26_facade,0,0,2)
    ZEND_ARG_TYPE_INFO(0, stringable, IS_OBJECT, 0)
    ZEND_ARG_INFO(0, callback)
ZEND_END_ARG_INFO()
PHP_FUNCTION(m26_facade_custody) {
    zval *stringable,*callback;
    ZEND_PARSE_PARAMETERS_START(2,2)
        Z_PARAM_OBJECT(stringable)
        Z_PARAM_ZVAL(callback)
    ZEND_PARSE_PARAMETERS_END();
    array_init(return_value);
    {
        zend::String first("first",5),second("second",6);
        first=second;
        add_assoc_stringl(return_value,"copy",first.val(),first.len());
        auto same=&first;first=*same;
        add_assoc_stringl(return_value,"self_copy",first.val(),first.len());
        first=std::move(second);
        add_assoc_bool(return_value,"move_source_null",second.get()==nullptr);
        first=std::move(*same);
        add_assoc_bool(return_value,"self_move_null",first.get()==nullptr);
        first=stringable;
        add_assoc_stringl(return_value,"conversion",first.val(),first.len());
        auto raw=zend_string_init("owned",5,false);
        zend::String retained(raw,true),taken(raw,false);
        add_assoc_stringl(return_value,"retained",retained.val(),retained.len());
        add_assoc_stringl(return_value,"taken",taken.val(),taken.len());
        zval duplicates;array_init(&duplicates);
        for(const auto &input: {std::string(),std::string("plain"),std::string("a\0B",3)}) {
            zend::String text(input.data(),input.size());
            char *native=text.dup(),*zend=text.len()?estrndup(text.val(),text.len()):nullptr;
            zval pair;array_init(&pair);
            if(native) {add_next_index_string(&pair,native);sw_free(native);}
            else add_next_index_null(&pair);
            if(zend) {add_next_index_stringl(&pair,zend,text.len());efree(zend);}
            else add_next_index_null(&pair);
            add_next_index_zval(&duplicates,&pair);
        }
        add_assoc_zval(return_value,"duplicates",&duplicates);
    }
    {
        zend::Variable source("value",5),assigned;
        assigned=source;
        zend::Variable copied(static_cast<const zend::Variable &&>(source));
        zend::Variable moved(std::move(source));
        add_assoc_bool(return_value,"variable_copy",Z_TYPE(assigned.value)==IS_STRING && Z_TYPE(copied.value)==IS_STRING);
        add_assoc_bool(return_value,"variable_move",Z_ISUNDEF(source.value) && Z_TYPE(moved.value)==IS_STRING);
    }
    {
        auto original=sw_callable_create(callback);
        if(!original) {add_assoc_bool(return_value,"callable_ready",false);return;}
        auto duplicate=original->dup();
        zval called;ZVAL_UNDEF(&called);
        bool success=duplicate->call(0,nullptr,&called);
        add_assoc_bool(return_value,"callable_ready",original->ready() && duplicate->ready() && success);
        if(Z_ISUNDEF(called))add_assoc_null(return_value,"called");else add_assoc_zval(return_value,"called",&called);
        delete duplicate;delete original;
    }
}
PHP_FUNCTION(m26_private_custody) {
    zval *object, *callback;
    ZEND_PARSE_PARAMETERS_START(2,2)
        Z_PARAM_OBJECT(object)
        Z_PARAM_ZVAL(callback)
    ZEND_PARSE_PARAMETERS_END();
    array_init(return_value);
    auto *scope = Z_OBJCE_P(object);
    auto *scalar = sw_zend_read_property(scope, object, ZEND_STRL("scalar"), 1);
    add_assoc_stringl(return_value, "scalar", Z_STRVAL_P(scalar), Z_STRLEN_P(scalar));
    auto *name = zend_string_init(ZEND_STRL("scalar"), false);
    auto *direct = sw_zend_read_property_ex(scope, object, name, 1);
    add_assoc_bool(return_value, "same_slot", direct == scalar);
    zend_string_release(name);
    auto *array = sw_zend_read_and_convert_property_array(scope, object, ZEND_STRL("container"), 1);
    add_assoc_long(return_value, "converted_count", zend_hash_num_elements(Z_ARRVAL_P(array)));
    auto *heap = sw_malloc_zval(); ZVAL_STRING(heap, "heap");
    auto *copy = sw_zval_dup(heap);
    add_assoc_bool(return_value, "shallow_copy", Z_STR_P(copy) == Z_STR_P(heap) && Z_REFCOUNT_P(heap) == 1);
    zval_ptr_dtor(heap); efree(heap); efree(copy);
    (void) callback;
}

ZEND_BEGIN_ARG_INFO_EX(arginfo_m26_oom,0,0,1)
    ZEND_ARG_TYPE_INFO(0, mode, IS_LONG, 0)
ZEND_END_ARG_INFO()
PHP_FUNCTION(m26_facade_oom) {
    zend_long mode;
    ZEND_PARSE_PARAMETERS_START(1,1) Z_PARAM_LONG(mode) ZEND_PARSE_PARAMETERS_END();
    if (PG(memory_limit) <= 0) {
        zend_throw_error(nullptr, "OOM witness requires a positive request memory limit");
        return;
    }
    const size_t limit = static_cast<size_t>(PG(memory_limit));
    const size_t live = zend_memory_usage(true);
    const size_t headroom = live < limit ? limit - live : 0;
    // Exceed the soft budget without reaching twice the limit, regardless of
    // request startup usage. The separate hard case still requests twice it.
    const size_t requested = mode == 3 ? limit * 2 : headroom + limit / 4;
    fprintf(stderr, "oom-input: mode=%ld limit=%zu live=%zu requested=%zu\n",
            static_cast<long>(mode), limit, live, requested);
    std::string input(requested, 'x');
    if(mode==1) { zend::Variable value(input); }
    else { zend::String value(input); }
    RETURN_TRUE;
}

static int class_alias_registered;
static zend_object_handlers guard_handlers;
static zend_object *guard_create(zend_class_entry *ce) {
    auto *object=static_cast<zend_object *>(zend_object_alloc(sizeof(zend_object),ce));
    zend_object_std_init(object,ce);
    object_properties_init(object,ce);
    object->handlers=&guard_handlers;
    return object;
}
ZEND_BEGIN_ARG_INFO_EX(arginfo_m26_utility,0,0,2)
    ZEND_ARG_INFO(0, callback)
    ZEND_ARG_TYPE_INFO(0, data, IS_OBJECT, 0)
ZEND_END_ARG_INFO()
PHP_FUNCTION(m26_utility_custody) {
    zval *callback, *data;
    ZEND_PARSE_PARAMETERS_START(2,2) Z_PARAM_ZVAL(callback) Z_PARAM_OBJECT(data) ZEND_PARSE_PARAMETERS_END();
    array_init(return_value);
    zend_fcall_info_cache cache{};
    char *name=nullptr, *error=nullptr;
    size_t length=0;
    bool callable=sw_zend_is_callable_at_frame(callback,nullptr,execute_data,0,&name,&length,&cache,&error);
    add_assoc_bool(return_value,"callable",callable);
    add_assoc_stringl(return_value,"name",name,length);
    efree(name);
    if(error) efree(error);
    size_t only_length=0;
    sw_zend_is_callable_at_frame(callback,nullptr,execute_data,0,nullptr,&only_length,&cache,nullptr);
    add_assoc_bool(return_value,"optional_name",only_length==length);
    (void) data;
    add_assoc_long(return_value,"class_alias_registered",class_alias_registered);
}
PHP_MINIT_FUNCTION(m26_facade) {
    (void) type; (void) module_number;
    guard_handlers=std_object_handlers;
    guard_handlers.unset_property=sw_zend_class_unset_property_deny;
    zend_class_entry entry;
    INIT_CLASS_ENTRY(entry,"M26Guard",nullptr);
    auto *guard=zend_register_internal_class(&entry);
    guard->create_object=guard_create;
    zend_declare_property_null(guard,ZEND_STRL("fixed"),ZEND_ACC_PUBLIC);
    class_alias_registered=sw_zend_register_class_alias(ZEND_STRL("M26GuArDAlias"),guard);
    return SUCCESS;
}
static const zend_function_entry functions[]={PHP_FE(m26_facade_custody,arginfo_m26_facade) PHP_FE(m26_private_custody,arginfo_m26_facade) PHP_FE(m26_facade_oom,arginfo_m26_oom) PHP_FE(m26_utility_custody,arginfo_m26_utility) PHP_FE_END};
extern "C" {
zend_module_entry m26_facade_module_entry={STANDARD_MODULE_HEADER,"m26_facade",functions,PHP_MINIT(m26_facade),nullptr,nullptr,nullptr,nullptr,"0",STANDARD_MODULE_PROPERTIES};
ZEND_GET_MODULE(m26_facade)
}
