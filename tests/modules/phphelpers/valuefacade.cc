#include "swoole_rs_phphelpers.h"
#include <cassert>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>
#include <utility>
struct Payload { std::string text; size_t refs; };
struct zval { Payload *payload=nullptr; };
static std::vector<std::string> events;
static std::vector<Payload *> allocations;
static Payload *allocate(const char *text,size_t length) { auto p=new Payload{std::string(text,length),1};allocations.push_back(p);events.push_back("allocate:"+p->text);return p; }
static void retain(zval *value) { if(value->payload) { assert(value->payload->refs);++value->payload->refs;events.push_back("retain:"+value->payload->text); } }
static void release(zval *value) { if(value->payload) { assert(value->payload->refs);--value->payload->refs;events.push_back("release:"+value->payload->text); } }
#define Z_TRY_ADDREF_P(value) retain(value)
#define Z_TRY_DELREF_P(value) release(value)
#define ZVAL_STRINGL(value,bytes,length) ((value)->payload=allocate(bytes,length))
#define ZVAL_STRING(value,bytes) ZVAL_STRINGL(value,bytes,std::strlen(bytes))
#define ZVAL_UNDEF(value) (*(value)=zval{})
#define zval_ptr_dtor(value) release(value)
namespace reference {
#include "reference-valuefacade.inc"
}
namespace candidate {
#include "candidate-valuefacade.inc"
}
template<class Variable> static std::vector<std::string> exercise(unsigned mode) {
    events.clear();
    zval source;source.payload=allocate("source",6);
    {
        Variable value(&source);
        if(mode==0) { Variable copy(static_cast<const Variable &&>(value));assert(copy.ptr()->payload==source.payload); }
        if(mode==1) { Variable moved(std::move(value));assert(moved.ptr()->payload==source.payload && value.ptr()->payload==nullptr); }
        if(mode==2) { Variable target;target=value;assert(target.ptr()->payload==source.payload); }
        if(mode==3) { Variable target;target.assign(&source);assert(target.ptr()->payload==source.payload); }
        if(mode==4) { auto same=&value;value=*same; }
    }
    release(&source);
    // Preserve source-defined self-copy's native extra retain; do not add a
    // destructor or self guard to production. The modeled pool remains owned.
    for(auto p:allocations) { events.push_back("remaining:"+std::to_string(p->refs));assert(p->refs==(mode==4?1u:0u));delete p; }
    allocations.clear();return events;
}
int main() {
    for(unsigned mode=0;mode<5;++mode) {
        const auto expected=exercise<reference::Variable>(mode);
        const auto actual=exercise<candidate::Variable>(mode);
        assert(expected==actual);
    }
    std::puts("{\"whole_classes\":1,\"matched_cases\":5}");
}
