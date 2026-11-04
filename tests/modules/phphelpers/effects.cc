// Complete pinned/selected helper bodies with equal ordered native effects.
// Fake Zend storage is deliberate: this proves prefixes, not Zend runtime custody.
#include <cassert>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <sstream>
#include <string>
#include "swoole_rs_phphelpers.h"
struct zend_array;
struct zval { int type=0, id=0, refs=1; zend_array *array=nullptr; };
struct zend_array { zval current; };
using HashTable=zend_array;
using zend_ulong=unsigned long;
struct zend_fcall_info_cache {};
struct EffectFailure {};
static std::ostringstream trace;
static int effect, fail_at;
static bool numeric, inserted, truthy, callable;
static void tick(const char *name) { trace<<name<<';';if(effect++==fail_at)throw EffectFailure{}; }
#define zend_always_inline inline
#define E_WARNING 2
#define Z_ARRVAL_P(p) ((p)->array)
static bool numeric_key(const char *,size_t,zend_ulong &index) {tick("numeric");index=42;return numeric;}
#define ZEND_HANDLE_NUMERIC_STR(p,n,i) ::numeric_key(p,n,i)
static zval *add(HashTable *ht) {tick("add");return inserted ? &ht->current : nullptr;}
static zval *find(HashTable *ht) {tick("find");return &ht->current;}
static zval *update(HashTable *ht) {tick("update");return &ht->current;}
static zval *zend_hash_index_add(HashTable *ht,zend_ulong index,zval *) {trace<<"index:add:"<<index<<';';return add(ht);}
static zval *zend_hash_str_add(HashTable *ht,const char *key,size_t length,zval *) {trace<<"string:add:"<<std::string(key,length)<<';';return add(ht);}
static zval *zend_hash_index_find(HashTable *ht,zend_ulong index) {trace<<"index:find:"<<index<<';';return find(ht);}
static zval *zend_hash_str_find(HashTable *ht,const char *key,size_t length) {trace<<"string:find:"<<std::string(key,length)<<';';return find(ht);}
static zval *zend_hash_index_update(HashTable *ht,zend_ulong index,zval *) {trace<<"index:update:"<<index<<';';return update(ht);}
static zval *zend_hash_str_update(HashTable *ht,const char *key,size_t length,zval *) {trace<<"string:update:"<<std::string(key,length)<<';';return update(ht);}
static bool is_array(zval *value) {tick("array-observation");return value->type==6;}
#define ZVAL_IS_ARRAY(p) is_array(p)
static void array_init_size(zval *value,size_t) {tick("array-init");value->type=6;}
static void addref(zval *value) {tick("addref");++value->refs;}
#define Z_ADDREF_P(p) addref(p)
static void add_next_index_zval(zval *,zval *value) {trace<<"append:"<<value->id<<';';tick("append");}
static void undef(zval *value) {tick("undef");value->type=0;}
#define ZVAL_UNDEF(p) undef(p)
static bool zval_is_true(zval *) {tick("truthy");return truthy;}
static bool sw_zend_is_callable_ex(zval *,void *,int,char **name,void *,zend_fcall_info_cache *,void *) {
    tick("callable");*name=const_cast<char *>("fixture-function");return callable;
}
static void warning(int,const char *,...) {tick("warning");}
#define php_swoole_fatal_error(...) warning(__VA_ARGS__)
static void zval_add_ref(zval *value) {addref(value);}
// Exact function::call bodies exercise return classification after native calls.
// Thrown sentinels model effect interruption only; they are not Zend bailout proof.
static int64_t call_result;
static void *exception;
static bool publish_exception, release_exception;
static void *function_table;
#define EG(name) name
#define UNEXPECTED(value) (value)
#define SUCCESS 0
#define E_ERROR 1
static void make_null(zval *value) {tick("return-null");value->type=1;}
#define ZVAL_NULL(value) make_null(value)
static int sw_zend_call_function_ex(void *,zend_fcall_info_cache *,uint32_t argc,zval *argv,zval *retval) {
    trace<<"direct:"<<argc<<':'<<argv[0].id<<':'<<(retval!=nullptr)<<';';tick("direct-call");
    if(retval)retval->type=4;
    if(publish_exception)exception=reinterpret_cast<void *>(uintptr_t(1));
    return static_cast<int>(call_result);
}
namespace swoole {
struct PHPCoroutine {
    static int64_t create(zend_fcall_info_cache *,uint32_t argc,zval *argv,void *) {
        trace<<"coroutine:"<<argc<<':'<<argv[0].id<<';';tick("coroutine-create");
        if(publish_exception)exception=reinterpret_cast<void *>(uintptr_t(1));
        return call_result;
    }
};
}
static void zend_exception_error(void *,int) {tick("exception-error");throw EffectFailure{};}
static void string_init(zval *value,const char *data,size_t length) {
    trace<<"name:"<<std::string(data,length)<<';';tick("name-allocate");value->type=5;
}
#define ZVAL_STRINGL(value,data,length) string_init(value,data,length)
static void zval_dtor(zval *value) {
    trace<<"dtor:"<<value->type<<';';tick("name-release");value->type=0;
    if(release_exception)exception=reinterpret_cast<void *>(uintptr_t(2));
}
struct Variable {
    zval value;
    Variable() {tick("variable-construct");value.type=0;}
    ~Variable() {trace<<"variable-release:"<<value.type<<';';}
};
static int call_user_function(void *,void *,zval *name,zval *retval,int argc,zval *argv) {
    trace<<"named:"<<name->type<<':'<<argc<<':'<<argv[0].id<<';';tick("named-call");retval->type=4;
    if(publish_exception)exception=reinterpret_cast<void *>(uintptr_t(1));
    return static_cast<int>(call_result);
}
namespace reference {
struct Callable { zval zfn;char *fn_name=nullptr;zend_fcall_info_cache fcc;explicit Callable(zval *); };
#include "reference-effects.inc"
}
namespace candidate {
struct Callable { zval zfn;char *fn_name=nullptr;zend_fcall_info_cache fcc;explicit Callable(zval *); };
#include "candidate-effects.inc"
}
template<class Callable> static std::string construct() {
    trace.str("");trace.clear();effect=0;zval input;input.type=7;input.id=9;
    try {Callable value(&input);trace<<"final:"<<value.zfn.type<<':'<<value.zfn.id<<':'<<value.zfn.refs<<';';}
    catch(const EffectFailure &) {trace<<"throw;";}
    return trace.str();
}
using Merge=void (*)(zval *,const char *,size_t,zval *);
static std::string merge(Merge fn,bool array) {
    trace.str("");trace.clear();effect=0;zend_array table;table.current.type=array ? 6 : 3;table.current.id=1;
    zval container;container.array=&table;zval added;added.id=2;
    try {fn(&container,"fixture",7,&added);trace<<"return;";}
    catch(const EffectFailure &) {trace<<"throw;";}
    trace<<"refs:"<<table.current.refs;return trace.str();
}
using Call=bool (*)(zend_fcall_info_cache *,uint32_t,zval *,zval *,bool);
static std::string call(Call fn,bool coroutine,bool return_storage) {
    trace.str("");trace.clear();effect=0;exception=nullptr;
    zval argv[2];argv[0].id=17;zval retval;retval.type=9;zend_fcall_info_cache cache;
    try {const bool result=fn(&cache,2,argv,return_storage?&retval:nullptr,coroutine);trace<<"return:"<<result<<';';}
    catch(const EffectFailure &) {trace<<"throw;";}
    trace<<"retval:"<<retval.type<<";exception:"<<(exception!=nullptr);return trace.str();
}
using NamedCall=Variable (*)(const std::string &,int,zval *);
static std::string named_call(NamedCall fn) {
    trace.str("");trace.clear();effect=0;exception=nullptr;
    zval argv[2];argv[0].id=19;
    try {auto value=fn(std::string("model\0function",14),2,argv);trace<<"return:"<<value.value.type<<';';}
    catch(const EffectFailure &) {trace<<"throw;";}
    trace<<"exception:"<<(exception!=nullptr);return trace.str();
}
int main() {
    size_t checks=0;
    for(bool num:{false,true})for(bool add_ok:{false,true})for(bool array:{false,true})for(int fail=-1;fail<12;++fail) {
        numeric=num;inserted=add_ok;fail_at=fail;
        const auto before=merge(reference::array_add_or_merge,array),after=merge(candidate::array_add_or_merge,array);
        if(before!=after){std::cerr<<before<<'\n'<<after<<'\n';return 1;}++checks;
    }
    for(bool first:{false,true})for(bool second:{false,true})for(int fail=-1;fail<7;++fail) {
        truthy=first;callable=second;fail_at=fail;
        const auto before=construct<reference::Callable>(),after=construct<candidate::Callable>();
        if(before!=after){std::cerr<<before<<'\n'<<after<<'\n';return 1;}++checks;
    }
    size_t calls=0;
    for(bool coroutine:{false,true})for(bool storage:{false,true})for(bool errors:{false,true})
        for(int64_t result:{int64_t(INT32_MIN),int64_t(-1),int64_t(0),int64_t(1),int64_t(INT32_MAX)})
            for(int fail=-1;fail<4;++fail) {
                call_result=result;publish_exception=errors;fail_at=fail;
                const auto before=call(reference::function::call,coroutine,storage),after=call(candidate::function::call,coroutine,storage);
                if(before!=after){std::cerr<<before<<'\n'<<after<<'\n';return 1;}++calls;
            }
    size_t named=0;
    for(bool errors:{false,true})for(bool release_errors:{false,true})
        for(int64_t result:{int64_t(INT32_MIN),int64_t(-1),int64_t(0),int64_t(1),int64_t(INT32_MAX)})
            for(int fail=-1;fail<7;++fail) {
                call_result=result;publish_exception=errors;release_exception=release_errors;fail_at=fail;
                const auto before=named_call(reference::function::call),after=named_call(candidate::function::call);
                if(before!=after){std::cerr<<before<<'\n'<<after<<'\n';return 1;}++named;
            }
    std::cout<<"{\"actual_call_effect_prefixes\":"<<calls<<",\"actual_named_call_effect_prefixes\":"<<named<<",\"actual_native_effect_prefixes\":"<<checks<<",\"zend_runtime_executed\":false}\n";
}
