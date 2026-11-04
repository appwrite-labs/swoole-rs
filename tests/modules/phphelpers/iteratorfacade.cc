#include "swoole_rs_phphelpers.h"
#include <cassert>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <string>
#include <vector>
using zend_ulong=unsigned long;
struct HashTable;
struct zend_resource { int id; };
struct zval { int type; int value; zval *indirect; HashTable *array; zend_resource *resource; };
struct zend_string { const char *val; size_t len; };
struct Bucket { zend_string *key; zval val; zend_ulong h; };
struct HashTable { Bucket *arData; uint32_t nNumUsed; uint32_t count; int status; };
static std::vector<int> events;
static int kind(const zval *value) { assert(value);events.push_back(100+value->type);return value->type; }
static zval *indirect(zval *value) { events.push_back(200);return value->indirect; }
static size_t zend_hash_num_elements(HashTable *table) { events.push_back(300);return table->count; }
static int add_index_zval(zval *array,zend_ulong index,zval *value) {events.push_back(400+static_cast<int>(index));events.push_back(value->type);return array->array->status;}
static int add_next_index_zval(zval *array,zval *value) {events.push_back(500);events.push_back(value->type);return array->array->status;}
#define IS_UNDEF 0
#define IS_INDIRECT 1
#define IS_ARRAY 2
#define IS_LONG 3
#define IS_RESOURCE 4
#define Z_TYPE_P(value) kind(value)
#define Z_INDIRECT_P(value) indirect(value)
#define Z_ARRVAL_P(value) ((value)->array)
#define ZVAL_RES(value,res) (*(value)=zval{IS_RESOURCE,0,nullptr,nullptr,res})
#define UNEXPECTED(value) (value)
#define SUCCESS 0
namespace reference {
#include "reference-iteratorfacade.inc"
}
namespace candidate {
#include "candidate-iteratorfacade.inc"
}
struct Output { std::vector<std::string> values; std::vector<int> effects; bool operator==(const Output &other) const{return values==other.values && effects==other.effects;} };
template<class Array> static Output walk(zval *container) {
    events.clear();std::vector<std::string> values;
    Array array(container);values.push_back("count:"+std::to_string(array.count()));
    for(auto it=array.begin(),end=array.end();it!=end;it++) {
        const auto offset=it.ptr()-container->array->arData;
        std::string key=container->array->arData[offset].key?it.key():"numeric";
        values.push_back(std::to_string(offset)+":"+std::to_string(it.value()->value)+":"+key+":"+std::to_string(it.index()));
    }
    return {values,events};
}
template<class Array> static Output update(zval *container) {
    events.clear();Array array(container);zval value{IS_LONG,9,nullptr,nullptr,nullptr};zend_resource resource{7};
    const bool a=array.set(2,&value),b=array.append(&value),c=array.set(4,&resource);
    assert(a==(container->array->status==0) && a==b && b==c);
    return {{a?"success":"failure"},events};
}
int main() {
    size_t cases=0;zend_string key{"key",3};
    // An initialized sentinel after nNumUsed preserves the pinned end-iterator
    // constructor's initial native field reads. Exact-capacity/OOM is not modeled.
    for(unsigned used=0;used<=4;++used) for(unsigned pattern=0;pattern<(1u<<(used*3));++pattern) {
        Bucket buckets[6]{};zval targets[4]{};uint32_t count=0;
        for(unsigned index=0;index<used;++index) {
            const unsigned mode=(pattern>>(index*3))&7;
            auto &bucket=buckets[index];bucket.h=10+index;bucket.key=(mode&4)?&key:nullptr;
            targets[index]={static_cast<int>((mode&3)==3?IS_LONG:IS_UNDEF),static_cast<int>(20+index),nullptr,nullptr,nullptr};
            bucket.val={static_cast<int>((mode&3)>=2?IS_INDIRECT:((mode&3)==1?IS_LONG:IS_UNDEF)),static_cast<int>(30+index),&targets[index],nullptr,nullptr};
            if((mode&3)==1 || (mode&3)==3)++count;
        }
        HashTable table{buckets,used,count,0};zval container{IS_ARRAY,0,nullptr,&table,nullptr};
        assert(walk<reference::Array>(&container)==walk<candidate::Array>(&container));++cases;
    }
    for(int status:{0,-1}) {Bucket buckets[2]{};HashTable table{buckets,0,0,status};zval container{IS_ARRAY,0,nullptr,&table,nullptr};assert(update<reference::Array>(&container)==update<candidate::Array>(&container));}
    std::printf("{\"whole_classes\":2,\"matched_walks\":%zu,\"matched_update_profiles\":2}\n",cases);
}
