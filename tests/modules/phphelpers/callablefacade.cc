#include "swoole_rs_phphelpers.h"
#include <cassert>
#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>
struct zend_refcounted { int refs; };
struct zval { zend_refcounted *payload; int kind; bool named; };
struct zend_fcall_info_cache { int marker; };
static std::vector<std::string> events;
static char *estrdup(const char *value) { events.push_back("duplicate-name");auto copy=static_cast<char *>(std::malloc(std::strlen(value)+1));std::strcpy(copy,value);return copy; }
static void retain(zval *value) { assert(value->payload);++value->payload->refs;events.push_back("retain-value"); }
static void construct(zval *destination,zend_fcall_info_cache *cache,char **name,zval *input) {
    assert(input);events.push_back("construct");*destination=*input;cache->marker=42;
    if(input->kind==1) {retain(destination);if(input->named)*name=estrdup("callable");}
    else destination->kind=0;
}
static void destroy(zval *value,char *name) { events.push_back("destroy");if(value->kind==1) {assert(value->payload->refs);--value->payload->refs;events.push_back("release-value");} if(name) {events.push_back("free-name");std::free(name);} }
static bool is_undefined(const zval *value) { events.push_back("observe-ready");return value->kind==0; }
static bool is_null(zval *value) { assert(value);events.push_back("observe-null");return value->kind==0; }
static zend_refcounted *sw_get_refcount_ptr(zval *value) { return value->payload; }
static int sw_zend_call_function_ex(zval *,zend_fcall_info_cache *,uint32_t argc,zval *,zval *) { events.push_back("invoke");return argc==0?0:-1; }
static const char *zend_zval_type_name(zval *value) { assert(value);events.push_back("observe-type");return "integer"; }
static void zend_throw_exception_ex(void *,int,const char *format,...) { char message[256];va_list args;va_start(args,format);std::vsnprintf(message,sizeof(message),format,args);va_end(args);events.push_back(message); }
static void *swoole_exception_ce=nullptr;
#define SW_ERROR_INVALID_PARAMS 1
#define SUCCESS 0
#define ZVAL_UNDEF(value) ((value)->kind=0)
#define ZVAL_IS_UNDEF(value) is_undefined(value)
#define ZVAL_IS_NULL(value) is_null(value)
#define zval_add_ref(value) retain(value)
namespace reference {
#include "reference-callablefacade.inc"
}
namespace candidate {
#include "candidate-callablefacade.inc"
}
template<class Callable,class Factory,class Nullable> static std::vector<std::string> exercise(unsigned mode,Factory create,Nullable nullable) {
    events.clear();zend_refcounted payload{1};zval input{&payload,1,mode%2!=0};
    if(mode<4) {
        {Callable original(&input);auto copy=original.dup();assert(copy->ptr()->marker==42);delete copy;}
    } else if(mode<6) {
        {Callable original(&input);assert(original.call(mode-4,nullptr,nullptr)==(mode==4));}
    } else {
        const bool allow=(mode%2)!=0;zval *pointer=&input;
        if(mode<8)pointer=nullptr;
        else if(mode<10)input.kind=0;
        else if(mode<12)input.kind=2;
        auto value=nullable(pointer,"handler",allow);
        assert((value!=nullptr)==(mode>=12));delete value;
        if(mode>=12) {value=create(&input);assert(value);delete value;}
    }
    assert(payload.refs==1);return events;
}
int main() {
    for(unsigned mode=0;mode<14;++mode) {
        const auto expected=exercise<reference::zend::Callable>(mode,reference::sw_callable_create,reference::sw_callable_create_ex);
        const auto actual=exercise<candidate::zend::Callable>(mode,candidate::sw_callable_create,candidate::sw_callable_create_ex);
        assert(expected==actual);
    }
    std::puts("{\"whole_classes\":1,\"factories\":2,\"matched_cases\":14}");
}
