// Normal initialized settings models. No Zend callback execution or native IO.
#include <cassert>
#include <climits>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <sstream>
#include <string>
#include <unordered_map>
#include <vector>
#ifdef PORTS_DRIVER
#include "swoole_rs_ports.h"
#endif
#include "config.inc"
#include "bounds.inc"
#include "data-head.inc"
#include "timer-min.inc"
// One instance shared by both translation units.
inline std::ostringstream trace;
inline bool observe;
template<class T> struct Field {
    const char *name; T value{};
    explicit Field(const char *n):name(n){}
    operator T() const {return value;}
    Field &operator=(T next) {value=next;if(observe)trace<<name<<':'<<+next<<';';return *this;}
};
#define FIELD(T,n) Field<T> n{#n}
struct ListenPort {
    FIELD(uint16_t,backlog);FIELD(int,socket_buffer_size);FIELD(int,kernel_socket_recv_buffer_size);FIELD(int,kernel_socket_send_buffer_size);
    FIELD(uint16_t,heartbeat_idle_time);FIELD(uint32_t,buffer_high_watermark);FIELD(uint32_t,buffer_low_watermark);
    FIELD(bool,open_tcp_nodelay);FIELD(int,tcp_defer_accept);FIELD(bool,open_tcp_keepalive);
    FIELD(bool,open_eof_check);FIELD(bool,open_http_protocol);FIELD(bool,open_websocket_protocol);FIELD(bool,open_http2_protocol);
    FIELD(bool,open_mqtt_protocol);FIELD(bool,open_redis_protocol);FIELD(double,max_idle_time);
    FIELD(int,tcp_keepidle);FIELD(int,tcp_keepinterval);FIELD(int,tcp_keepcount);FIELD(int,tcp_user_timeout);FIELD(int,tcp_fastopen);FIELD(bool,open_length_check);
    struct {
        FIELD(bool,split_by_eof);FIELD(uint8_t,package_eof_len);char package_eof[SW_DATA_EOF_MAXLEN]{};
        FIELD(char,package_length_type);FIELD(uint8_t,package_length_size);FIELD(uint16_t,package_length_offset);FIELD(uint16_t,package_body_offset);
        FIELD(uint32_t,package_max_length);void *private_data_1=nullptr;void (*get_package_length)()=nullptr;
    } protocol;
};
struct Server { bool single_thread=false; };
struct ServerPortProperty { Server *serv; };
using zend_long=int64_t;
struct zval { std::string key,bytes; int64_t number=0;double real=0;bool truth=false,is_null=false; };
using HashTable=std::unordered_map<std::string,zval>;
static zval *zend_hash_str_find(HashTable *table,const char *name,size_t length) {
    std::string key(name,length);trace<<"lookup:"<<key<<';';auto i=table->find(key);return i==table->end()?nullptr:&i->second;
}
#define ZVAL_IS_NULL(p) ((p)->is_null)
#define php_swoole_array_get_value(ht,str,v) ((v=zend_hash_str_find(ht,str,sizeof(str)-1))&&!ZVAL_IS_NULL(v))
static bool zval_is_true(zval *v){trace<<"bool:"<<v->key<<';';return v->truth;}
static zend_long zval_get_long(zval *v){trace<<"long:"<<v->key<<';';return v->number;}
static zend_long php_swoole_parse_to_size(zval *v){trace<<"size:"<<v->key<<';';return v->number;}
static double zval_get_double(zval *v){trace<<"double:"<<v->key<<';';return v->real;}
namespace zend {
struct String {std::string bytes;explicit String(zval *v):bytes(v->bytes){trace<<"string:"<<v->key<<';';}size_t len()const{return bytes.size();}const char *val()const{return bytes.data();}};
}
static void php_swoole_server_set_websocket_option(ListenPort *,HashTable *){trace<<"websocket-helper;";}
static void *sw_callable_create(zval *){assert(false&&"callable settings omitted from normal inventory");return nullptr;}
static void sw_callable_free(void *){assert(false&&"no callback lifecycle profiles");}
[[maybe_unused]] static void php_swoole_server_length_func(){}
struct Protocol { using LengthFunc = void (*)(); };
[[maybe_unused]] static void swr_v1_phpserver_length_func(){}
#define E_ERROR 1
#define RETURN_FALSE return false
static void php_swoole_fatal_error(int,const char *,...){assert(false&&"invalid/fatal settings omitted");}
namespace original { bool set_prefix(ListenPort *port, ServerPortProperty *property, HashTable *vht); }
namespace selected { bool set_prefix(ListenPort *port, ServerPortProperty *property, HashTable *vht); }
// Each side's body in its own translation unit; the selected one also holds the driver.
namespace PORTS_SIDE {
#include "type-size.inc"
#include "apply.inc"
#include "body.inc"
}
#ifdef PORTS_DRIVER
static uint32_t seed=0x91281;
static uint32_t random_word(){seed^=seed<<13;seed^=seed>>17;seed^=seed<<5;return seed;}
static const char *keys[]={"backlog","socket_buffer_size","kernel_socket_recv_buffer_size","kernel_socket_send_buffer_size","heartbeat_idle_time","buffer_high_watermark","buffer_low_watermark","open_tcp_nodelay","tcp_defer_accept","open_tcp_keepalive","open_eof_check","open_eof_split","package_eof","open_http_protocol","open_websocket_protocol","open_http2_protocol","open_mqtt_protocol","open_redis_protocol","max_idle_time","tcp_keepidle","tcp_keepinterval","tcp_keepcount","tcp_user_timeout","tcp_fastopen","open_length_check","package_length_type","package_length_offset","package_body_offset","package_body_start","package_max_length"};
static HashTable settings(unsigned sample,bool binary=false) {
    HashTable table;
    for(const char *key:keys) {
        auto word=random_word();if((word&3)==0)continue;
        zval v;v.key=key;v.is_null=(word&15)==1;v.truth=(word&32)!=0;
        const int64_t values[]={INT64_MIN,INT64_MAX,-1,0,1,UINT16_MAX,UINT32_MAX};v.number=values[(word>>8)%7];
        v.real=((word>>16)%1000)/100.0;
        v.bytes=std::string(1+(word%SW_DATA_EOF_MAXLEN),char('a'+sample%26));
        if(binary)for(size_t n=0;n<v.bytes.size();n++)v.bytes[n]=static_cast<char>((word>>((n%4)*8))&255);
        if(v.key=="package_length_type")v.bytes="N";
        if(v.key=="package_length_offset"||v.key=="package_body_offset"||v.key=="package_body_start")v.number=word%SW_IPC_BUFFER_SIZE;
        table.emplace(key,v);
    }
    return table;
}
static void initialize(ListenPort &p,unsigned sample) {
    observe=false;
    p.open_tcp_nodelay=sample&1;p.open_tcp_keepalive=(sample>>1)&1;p.open_eof_check=(sample>>2)&1;p.protocol.split_by_eof=(sample>>3)&1;
    p.open_http_protocol=(sample>>4)&1;p.open_websocket_protocol=(sample>>5)&1;p.open_http2_protocol=(sample>>6)&1;
    p.open_mqtt_protocol=(sample>>7)&1;p.open_redis_protocol=(sample>>8)&1;p.tcp_fastopen=(sample>>9)&1;p.open_length_check=(sample>>10)&1;
}
static std::string run(bool selected,unsigned sample,HashTable input) {
    trace.str("");trace.clear();ListenPort p;initialize(p,sample);Server server;ServerPortProperty property{&server};observe=true;
    bool result=selected?selected::set_prefix(&p,&property,&input):original::set_prefix(&p,&property,&input);
    observe=false;trace<<"result:"<<result<<";eof:";for(char c:p.protocol.package_eof)trace<<unsigned(static_cast<unsigned char>(c))<<',';
    trace<<";flags:"<<p.open_tcp_nodelay.value<<p.open_tcp_keepalive.value<<p.open_eof_check.value<<p.protocol.split_by_eof.value<<p.open_http_protocol.value<<p.open_websocket_protocol.value<<p.open_http2_protocol.value<<p.open_mqtt_protocol.value<<p.open_redis_protocol.value<<p.tcp_fastopen.value<<p.open_length_check.value;
    return trace.str();
}
static size_t flag_checks(bool binary=false) {
    static_assert(SWR_PORTS_SET_NODELAY==9&&SWR_PORTS_SET_CHECK_LENGTH==19,"flag field IDs");
    size_t cases=0;
    for(unsigned sample=0;sample<4096;sample++) {
        auto input=settings(sample,binary);auto expected=run(false,sample,input);auto actual=run(true,sample,input);
        if(expected!=actual){fprintf(stderr,"flag prefix trace mismatch sample=%u\nEXPECTED %s\nACTUAL %s\n",sample,expected.c_str(),actual.c_str());return 1;}cases++;
    }
    for(uint32_t field=SWR_PORTS_SET_NODELAY;field<=SWR_PORTS_SET_CHECK_LENGTH;field++)
        for(uint32_t present:{0u,1u})for(int64_t value:{INT64_MIN,int64_t(-1),int64_t(0),int64_t(1),INT64_MAX}) {
            auto plan=swr_v1_ports_flag_setting(field,value,present);assert(plan.count<=2);
            for(uint32_t n=plan.count;n<6;n++)assert(plan.actions[n].kind==0&&plan.actions[n].value==0);
            if(present==0)assert(plan.count==(field==SWR_PORTS_SET_NODELAY?1u:0u));
            if(plan.count)assert(plan.actions[0].kind==field&&plan.actions[0].value==uint32_t(present?value!=0:true));
            if(plan.count==2)assert(plan.actions[1].value==1);
            cases++;
        }
    static_assert(sizeof(SwrPortsBufferSetting)==12,"signed buffer plan ABI");
    for(int64_t value:{INT64_MIN,int64_t(INT32_MIN),int64_t(-1),int64_t(0),int64_t(1),int64_t(INT32_MAX),INT64_MAX}) {
        auto plan=swr_v1_ports_buffer_setting(value);
        int32_t first=SW_MAX(INT_MIN,SW_MIN(value,INT_MAX));
        assert(plan.first==first&&plan.count==(first<=0?2u:1u));
        assert(plan.last==(first<=0?INT_MAX:0));cases++;
    }
    return cases;
}
#ifndef SWR_PORTS_FLAGS_NO_MAIN
int main() {
    printf("%zu normal exact-settings-prefix and direct plan comparisons passed\n",flag_checks());
    return 0;
}
#endif
#endif
