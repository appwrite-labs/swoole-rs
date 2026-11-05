// Complete known-string initialization/publication bodies.
// Native persistent/interned storage and value destruction are controlled doubles.
#include <cassert>
#include <cstdint>
#include <cstring>
#include <deque>
#include <iostream>
#include <string>
#include <unordered_map>
#include <vector>
#include "swoole_rs_phphelpers.h"
#include "known-macro.inc"
struct zend_string {std::string value;size_t identity;};
struct zval {int type;};
struct Interrupted {};
static std::vector<std::string> events;
static std::vector<zend_string *> table;
static std::deque<zend_string> strings;
static std::unordered_map<std::string,zend_string *> interned;
static zend_string ***active_cache;
static int effect,fail_at;
static bool deduplicate;
static void tick(const std::string &event) {events.push_back(event);if(effect++==fail_at)throw Interrupted{};}
static void *pemalloc(size_t size,int persistent) {assert(persistent==1);tick("persistent-allocate:"+std::to_string(size));table.assign(size/sizeof(zend_string *),nullptr);return table.data();}
static void pefree(void *data,int persistent) {assert(persistent==1&&data==table.data());tick("persistent-free:"+std::to_string(*active_cache==table.data()));}
static zend_string *zend_string_init(const char *value,size_t length,bool persistent) {
    assert(persistent);tick("string:"+std::string(value,length)+":published:"+std::to_string(*active_cache==table.data()));
    strings.push_back({std::string(value,length),strings.size()+1});return &strings.back();
}
static zend_string *zend_new_interned_string(zend_string *value) {
    tick("intern:"+value->value);
    if(!deduplicate)return value;
    auto found=interned.find(value->value);if(found!=interned.end())return found->second;
    interned.emplace(value->value,value);return value;
}
namespace reference {
#include "reference-cache.inc"
}
namespace candidate {
#include "candidate-cache.inc"
}
using Function=void (*)();
static std::vector<std::string> known(Function init,Function destroy,zend_string **&cache,int failure,bool dedup) {
    events.clear();strings.clear();interned.clear();table.clear();effect=0;fail_at=failure;deduplicate=dedup;cache=nullptr;active_cache=&cache;
    if(dedup) {
#define SEED_KNOWN(name,value) strings.push_back({value,strings.size()+1});interned.emplace(value,&strings.back());
        SW_ZEND_KNOWN_STRINGS(SEED_KNOWN)
#undef SEED_KNOWN
    }
    try {init();events.push_back("initialized");
        assert(table.size()==SW_ZEND_STR_LAST_KNOWN);
        for(size_t index=0;index<table.size();++index)events.push_back("slot:"+std::to_string(index)+":"+table[index]->value+":"+std::to_string(table[index]->identity));
        destroy();events.push_back("destroyed");}
    catch(const Interrupted &){events.push_back("interrupted");}
    events.push_back("cache-null:"+std::to_string(cache==nullptr));
    if(cache)for(size_t index=0;index<table.size();++index)events.push_back("prefix:"+std::to_string(index)+":"+(cache[index]?cache[index]->value:"unpublished"));
    return events;
}
int main() {
    size_t caches=0;
    for(bool dedup:{false,true})for(int failure=-1;failure<132;++failure) {
        const auto before=known(reference::known_strings_init,reference::known_strings_dtor,reference::sw_zend_known_strings,failure,dedup);
        const auto after=known(candidate::known_strings_init,candidate::known_strings_dtor,candidate::sw_zend_known_strings,failure,dedup);
        if(before!=after){for(auto &e:before)std::cerr<<"native "<<e<<'\n';for(auto &e:after)std::cerr<<"rust "<<e<<'\n';return 1;}++caches;
    }
    assert(swr_v1_phphelpers_known_count()==SW_ZEND_STR_LAST_KNOWN);
    std::cout<<"{\"known_strings\":"<<SW_ZEND_STR_LAST_KNOWN<<",\"actual_cache_effect_cases\":"<<caches<<",\"zend_runtime_executed\":false}\n";
}
