#include "swoole_rs_phphelpers.h"
#include <cassert>
#include <cctype>
#include <cstdlib>
#include <cstring>
#include <cstdio>
#include <string>
#include <vector>
#include <utility>
struct zend_string { std::string bytes; size_t refs; size_t len; };
struct zval { const char *bytes; };
static std::vector<std::string> events;
static std::vector<zend_string *> allocated;
static zend_string *zend_string_init(const char *bytes,size_t length,bool) {
    events.push_back("allocate:"+std::string(bytes,length));
    auto value=new zend_string{std::string(bytes,length),1,length};allocated.push_back(value);return value;
}
static zend_string *zend_string_copy(zend_string *value) { assert(value && value->refs);events.push_back("retain:"+value->bytes);++value->refs;return value; }
static void zend_string_release(zend_string *value) { assert(value && value->refs);events.push_back("release:"+value->bytes);--value->refs; }
static zend_string *zval_get_string(zval *value) { events.push_back("convert");return zend_string_init(value->bytes,std::strlen(value->bytes),false); }
static char *duplicate(const char *bytes,size_t length,const char *family) { events.push_back(std::string(family)+":"+std::string(bytes,length));auto value=static_cast<char *>(std::malloc(length+1));std::memcpy(value,bytes,length);value[length]=0;return value; }
static char *estrndup(const char *bytes,size_t length) { return duplicate(bytes,length,"zend-duplicate"); }
static char *zend_str_tolower_dup(const char *bytes,size_t length) { auto value=estrndup(bytes,length);for(size_t i=0;i<length;++i)value[i]=static_cast<char>(std::tolower(static_cast<unsigned char>(value[i])));events.push_back("lower");return value; }
static char *sw_strndup(const char *bytes,size_t length) {
    events.push_back("native-duplicate:"+std::string(bytes,length));
    size_t prefix=0;while(prefix<length && bytes[prefix])++prefix;
    auto result=static_cast<char *>(std::malloc(prefix+1));std::memcpy(result,bytes,prefix);result[prefix]=0;return result;
}
static void efree(char *value) { events.push_back("free:"+std::string(value));std::free(value); }
namespace swoole { static size_t rtrim(char *,size_t length) { return length; } }
#define ZSTR_VAL(value) const_cast<char *>((value)->bytes.data())
#define ZSTR_LEN(value) ((value)->len)
#define sw_likely(value) (value)
static size_t phase_dispatches,count_dispatches;
static uint32_t traced_string_plan(uint32_t *phase,uint32_t operation,uint8_t observation) {
    ++phase_dispatches;return swr_v1_phphelpers_string_plan(phase,operation,observation);
}
static uint8_t traced_empty(size_t count) { ++count_dispatches;return swr_v1_phphelpers_empty(count); }
#define swr_v1_phphelpers_string_plan traced_string_plan
#define swr_v1_phphelpers_empty traced_empty
namespace reference {
#include "reference-facade.inc"
}
namespace candidate {
#include "candidate-facade.inc"
}
template<class String,class CharPtr> static std::vector<std::string> exercise(unsigned mode) {
    events.clear();phase_dispatches=count_dispatches=0;
    {
        String source("source",6), target("target",6);
        zval value{"converted"};
        switch(mode) {
        case 0: target=source;break;
        case 1: {auto same=&target;target=*same;break;}
        case 2: target=std::move(source);break;
        case 3: {auto same=&target;target=std::move(*same);break;}
        case 4: target=&value;break;
        case 5: source.release();source.release();break;
        case 6: {String copy(source.get(),true);assert(copy.get()==source.get());break;}
        case 7: {auto raw=zend_string_init("owned",5,false);String take(raw,false);assert(take.get()==raw);break;}
        case 8: {CharPtr bytes;bytes.release();bytes.assign("AbC",3);bytes.assign_tolower("DeF",3);assert(std::strcmp(bytes.get(),"def")==0);bytes.release();bytes.release();break;}
        case 9: {CharPtr bytes("first");bytes.assign("second",6);assert(std::strcmp(bytes.get(),"second")==0);break;}
        case 10: case 11: case 12: {
            const char binary[]={'a',0,'B'};
            const char *input=mode==10?"":mode==11?"plain":binary;
            const size_t length=mode==10?0:mode==11?5:sizeof(binary);
            String bytes(input,length);
            const auto before=events.size();
            char *native=bytes.dup(),*zend=bytes.edup();
            if(!length) {assert(!native && !zend);assert(events.size()==before);}
            else {
                assert(native && zend && native!=zend);
                const size_t native_length=mode==12?1:length;
                assert(std::memcmp(native,input,native_length)==0 && native[native_length]==0);
                assert(std::memcmp(zend,input,length)==0 && zend[length]==0);
                assert(events[before]==std::string("native-duplicate:")+std::string(input,length));
                assert(events[before+1]==std::string("zend-duplicate:")+std::string(input,length));
                std::free(native);efree(zend);
            }
            break;
        }
        }
    }
    for(auto value:allocated) { assert(value->refs==0);delete value; }
    allocated.clear();return events;
}
int main() {
    size_t phases=0,counts=0;
    for(unsigned mode=0;mode<13;++mode) {
        const auto expected=exercise<reference::String,reference::CharPtr>(mode);
        assert(phase_dispatches==0 && count_dispatches==0);
        const auto actual=exercise<candidate::String,candidate::CharPtr>(mode);
        phases+=phase_dispatches;counts+=count_dispatches;
        assert(actual==expected);
    }
    std::printf("{\"whole_classes\":2,\"matched_cases\":13,\"phase_dispatches\":%zu,\"count_dispatches\":%zu,\"total_dispatches\":%zu}\n",phases,counts,phases+counts);
}
