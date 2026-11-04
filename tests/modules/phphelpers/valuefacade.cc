#include "swoole_rs_phphelpers.h"
#include <cassert>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>
#include <utility>
using zend_ulong=unsigned long;
struct Payload { std::string text; size_t refs; };
struct HashTable;
struct zval { Payload *payload=nullptr; zval *reference=nullptr; HashTable *array=nullptr; };
struct HashTable { zval slot; };
struct zend_string { size_t refs; };
static std::vector<std::string> events;
static std::vector<Payload *> allocations;
static Payload *allocate(const char *text,size_t length) { auto p=new Payload{std::string(text,length),1};allocations.push_back(p);events.push_back("allocate:"+p->text);return p; }
static zend_string *zend_string_copy(zend_string *p) { ++p->refs;events.push_back("retain-key");return p; }
static void zend_string_release(zend_string *p) { assert(p->refs);--p->refs;events.push_back("release-key"); }
static void retain(zval *value) { if(value->payload) { assert(value->payload->refs);++value->payload->refs;events.push_back("retain:"+value->payload->text); } }
static void release(zval *value) { if(value->payload) { assert(value->payload->refs);--value->payload->refs;events.push_back("release:"+value->payload->text); } }
static void dereference(zval *&value) { events.push_back("dereference");if(value->reference)value=value->reference; }
static zval *zend_hash_index_update(HashTable *table,zend_ulong,zval *value) { events.push_back("insert-index");release(&table->slot);table->slot=*value;return &table->slot; }
static zval *zend_hash_update(HashTable *table,zend_string *,zval *value) { events.push_back("insert-string");release(&table->slot);table->slot=*value;return &table->slot; }
#define Z_ARRVAL_P(value) ((value)->array)
#define ZVAL_DEREF(value) dereference(value)
#define Z_TRY_ADDREF(value) retain(&(value))
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
template<class KeyValue,class Variable> static std::vector<std::string> exercise(unsigned mode) {
    events.clear();
    zval source;source.payload=allocate("source",6);
    if(mode<4) {
        HashTable table;zval array;array.array=&table;
        zend_string key{1};zval indirect;indirect.reference=&source;
        if(mode>=2)table.slot.payload=allocate("old",3);
        {
            KeyValue value(7,mode%2?&key:nullptr,mode>=2?&indirect:&source);
            assert(value.zvalue.payload==source.payload);
            value.add_to(&array);
            assert(table.slot.payload==source.payload);
        }
        assert(key.refs==1);release(&source);release(&table.slot);
    } else {
        {
            Variable value(&source);
            if(mode==4) { Variable copy(static_cast<const Variable &&>(value));assert(copy.ptr()->payload==source.payload); }
            if(mode==5) { Variable moved(std::move(value));assert(moved.ptr()->payload==source.payload && value.ptr()->payload==nullptr); }
            if(mode==6) { Variable target;target=value;assert(target.ptr()->payload==source.payload); }
            if(mode==7) { Variable target;target.assign(&source);assert(target.ptr()->payload==source.payload); }
            if(mode==8) { auto same=&value;value=*same; }
        }
        release(&source);
    }
    // Preserve source-defined self-copy's native extra retain; do not add a
    // destructor or self guard to production. The modeled pool remains owned.
    for(auto p:allocations) { events.push_back("remaining:"+std::to_string(p->refs));assert(p->refs==(mode==8?1u:0u));delete p; }
    allocations.clear();return events;
}
int main() {
    for(unsigned mode=0;mode<9;++mode) {
        const auto expected=exercise<reference::KeyValue,reference::Variable>(mode);
        const auto actual=exercise<candidate::KeyValue,candidate::Variable>(mode);
        assert(expected==actual);
    }
    std::puts("{\"whole_classes\":2,\"matched_cases\":9}");
}
