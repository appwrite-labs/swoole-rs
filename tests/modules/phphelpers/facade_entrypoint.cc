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
            char *native=text.dup(),*zend=text.edup();
            zval pair;array_init(&pair);
            if(native) {add_next_index_string(&pair,native);sw_free(native);}
            else add_next_index_null(&pair);
            if(zend) {add_next_index_stringl(&pair,zend,text.len());efree(zend);}
            else add_next_index_null(&pair);
            add_next_index_zval(&duplicates,&pair);
        }
        add_assoc_zval(return_value,"duplicates",&duplicates);
        zend::CharPtr bytes("first");bytes.assign("second",6);bytes.assign_tolower("ABC",3);
        add_assoc_string(return_value,"lower",bytes.get());bytes.release();bytes.release();
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
        zval array;array_init(&array);
        auto key=zend_string_init("entry",5,false);
        for(const char *text:{"first","second"}) {
            zval value;ZVAL_STRING(&value,text);
            {zend::KeyValue entry(0,key,&value);entry.add_to(&array);}
            zval_ptr_dtor(&value);
        }
        zval value;ZVAL_STRING(&value,"last");
        {zend::KeyValue entry(7,nullptr,&value);entry.add_to(&array);}
        zval_ptr_dtor(&value);zend_string_release(key);
        add_assoc_string(&array,"drop","removed");
        zend_hash_str_del(Z_ARRVAL(array),ZEND_STRL("drop"));
        zend::Array facade(&array);
        zval inserted;ZVAL_STRING(&inserted,"set");
        bool set=facade.set(9,&inserted);if(set)ZVAL_UNDEF(&inserted);else zval_ptr_dtor(&inserted);
        ZVAL_STRING(&inserted,"append");
        bool append=facade.append(&inserted);if(append)ZVAL_UNDEF(&inserted);else zval_ptr_dtor(&inserted);
        add_assoc_bool(return_value,"updates",set && append);
        zval walk;array_init(&walk);
        // Borrow native payloads while their owning array stays live. The
        // initialized private sentinel gives both pinned constructors their
        // documented modeled input domain; do not probe unused native buckets.
        const auto used=Z_ARRVAL(array)->nNumUsed;
        std::vector<Bucket> buckets(used+1);
        memcpy(buckets.data(),Z_ARRVAL(array)->arData,used*sizeof(Bucket));
        for(zend::ArrayIterator it(buckets.data(),buckets.data()+used),end(buckets.data()+used);it!=end;it++) {
            zval entry;array_init(&entry);
            if(it.ptr()->key) {auto name=it.key();add_assoc_stringl(&entry,"key",name.data(),name.size());}
            else add_assoc_long(&entry,"key",it.index());
            zval copied;ZVAL_COPY(&copied,it.value());add_assoc_zval(&entry,"value",&copied);
            add_next_index_zval(&walk,&entry);
        }
        add_assoc_long(return_value,"array_count",facade.count());
        add_assoc_zval(return_value,"walk",&walk);
        add_assoc_zval(return_value,"map",&array);
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
        auto nullable=sw_callable_create_ex(callback,"callback",true);
        add_assoc_bool(return_value,"nullable_ready",nullable && nullable->ready());delete nullable;
        add_assoc_bool(return_value,"nullable_null",sw_callable_create_ex(nullptr,"callback",true)==nullptr);
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
    name = zend_string_init(ZEND_STRL("optional"), false);
    add_assoc_bool(return_value, "nullable", sw_zend_read_property_not_null_ex(scope, object, name, 1) == nullptr);
    zend_update_property_string(scope, Z_OBJ_P(object), ZEND_STRL("optional"), "ready");
    auto *optional = sw_zend_read_property_not_null_ex(scope, object, name, 1);
    add_assoc_stringl(return_value, "nonnull", Z_STRVAL_P(optional), Z_STRLEN_P(optional));
    zend_string_release(name);
    auto *array = sw_zend_read_and_convert_property_array(scope, object, ZEND_STRL("container"), 1);
    add_assoc_long(return_value, "converted_count", zend_hash_num_elements(Z_ARRVAL_P(array)));
    add_next_index_long(array, 7);
    array = sw_zend_update_and_read_property_array(scope, object, ZEND_STRL("container"));
    add_assoc_long(return_value, "reset_count", zend_hash_num_elements(Z_ARRVAL_P(array)));
    auto *heap = sw_malloc_zval(); ZVAL_STRING(heap, "heap");
    auto *copy = sw_zval_dup(heap);
    add_assoc_bool(return_value, "shallow_copy", Z_STR_P(copy) == Z_STR_P(heap) && Z_REFCOUNT_P(heap) == 1);
    Z_TRY_ADDREF_P(copy); sw_zval_free(copy);
    add_assoc_long(return_value, "after_copy_free", Z_REFCOUNT_P(heap));
    sw_zval_free(heap);
    zval args[2]; ZVAL_STRINGL(&args[0], "a\0B", 3); ZVAL_LONG(&args[1], 19);
    zend_fcall_info frame{}; frame.param_count=2; frame.params=args;
    sw_zend_fci_params_persist(&frame);
    add_assoc_bool(return_value, "persisted", frame.params != args && Z_STR_P(&frame.params[0]) == Z_STR_P(&args[0]) && Z_LVAL(frame.params[1]) == 19);
    add_assoc_long(return_value, "persisted_refs", Z_REFCOUNT(args[0]));
    sw_zend_fci_params_discard(&frame);
    add_assoc_long(return_value, "released_refs", Z_REFCOUNT(args[0]));
    zval_ptr_dtor(&args[0]);
    zend_fcall_info info{}; zend_fcall_info_cache cache{};
    if (zend_fcall_info_init(callback, 0, &info, &cache, nullptr, nullptr) != SUCCESS) RETURN_FALSE;
    auto before=Z_REFCOUNT_P(callback);
    sw_zend_fci_cache_persist(&cache);
    add_assoc_long(return_value, "cache_retained", Z_REFCOUNT_P(callback)-before);
    sw_zend_fci_cache_discard(&cache);
    add_assoc_bool(return_value, "cache_released", Z_REFCOUNT_P(callback)==before);
    zval owned; object_init_ex(&owned, Z_OBJCE_P(callback));
    if (zend_fcall_info_init(&owned, 0, &info, &cache, nullptr, nullptr) != SUCCESS) RETURN_FALSE;
    sw_zend_fci_cache_persist(&cache);
    zval_ptr_dtor(&owned); ZVAL_UNDEF(&owned);
    sw_zend_fci_cache_discard(&cache);
    auto *shortened=zend_string_init("ABCxxxxx",8,false);
    shortened=sw_zend_string_recycle(shortened,8,3);
    add_assoc_stringl(return_value,"recycled",ZSTR_VAL(shortened),ZSTR_LEN(shortened));
    zend_string_release(shortened);
    auto length=2*swoole_pagesize()+16;
    auto *large=zend_string_alloc(length,false); memcpy(ZSTR_VAL(large),"XYZ",4);
    large=sw_zend_string_recycle(large,length,3);
    add_assoc_stringl(return_value,"shrunk",ZSTR_VAL(large),ZSTR_LEN(large));
    zend_string_release(large);
}

ZEND_BEGIN_ARG_INFO_EX(arginfo_m26_oom,0,0,1)
    ZEND_ARG_TYPE_INFO(0, mode, IS_LONG, 0)
ZEND_END_ARG_INFO()
PHP_FUNCTION(m26_facade_oom) {
    zend_long mode;
    ZEND_PARSE_PARAMETERS_START(1,1) Z_PARAM_LONG(mode) ZEND_PARSE_PARAMETERS_END();
    std::string input((mode==3 ? 32 : 24)*1024*1024, 'x');
    if(mode==0) { zend::String value(input); }
    else if(mode==1) { zend::Variable value(input); }
    else { zend::CharPtr value(input.data(),input.size()); }
    RETURN_TRUE;
}
static const zend_function_entry functions[]={PHP_FE(m26_facade_custody,arginfo_m26_facade) PHP_FE(m26_private_custody,arginfo_m26_facade) PHP_FE(m26_facade_oom,arginfo_m26_oom) PHP_FE_END};
extern "C" {
zend_module_entry m26_facade_module_entry={STANDARD_MODULE_HEADER,"m26_facade",functions,nullptr,nullptr,nullptr,nullptr,nullptr,"0",STANDARD_MODULE_PROPERTIES};
ZEND_GET_MODULE(m26_facade)
}
