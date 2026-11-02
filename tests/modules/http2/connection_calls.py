#!/usr/bin/env python3
"""Compare complete actual HTTP2 client connect/close bodies with native lifetime effects."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gate
import sides
from settings_calls import body, select_overlay


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--core-report', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--include', action='append', default=[])
    args = parser.parse_args()
    core = json.loads(args.core_report.read_text())
    if not core['passed']:
        raise ValueError('Completed passing core receipt required')
    artifacts = args.core_report.parent / (args.core_report.stem + '-artifacts')
    archive = Path(core.get('archive', artifacts / 'libhttp2.a'))
    header = gate.ROOT / 'native/http2/rust_http2.h'
    name = 'ext-src/swoole_http2_client_coro.cc'
    selected = artifacts / 'overlay' / name
    if gate.digest(archive) != core['archive_sha256'] or gate.digest(selected) != core['patched_sources'][name]:
        raise ValueError('Frozen archive/overlay identity changed')
    if gate.digest(header) != core['module_header_sha256']:
        raise ValueError('Module header changed')
    original = subprocess.check_output(['git', 'show', f'{gate.PIN}:{name}'], cwd=args.source, text=True)
    report = {'passed': False, 'scope': __doc__, 'upstream_commit': gate.PIN,
              'commands': [], 'gaps': [
                  'Real PHP references/properties/GC, actual sockets/HPACK/coroutine and Zend longjmp',
                  'Socket creation, settings, HPACK and sends are scripted native effects',
                  'Callbacks freeing a still-used raw client/socket are outside defined fixture lifetimes',
                  'Rust sanitizer instrumentation and optimized performance']}
    pieces = []
    for variant, text in [('native', original), ('rust', select_overlay(selected.read_text()))]:
        methods = []
        for signature in ['bool Client::connect()', 'bool Client::close() const']:
            fragment = body(text, signature)
            methods.append(signature.replace('Client::', '') + '{' + fragment + '}')
        pieces.append((variant, 'namespace ' + variant + ' {struct Client:FixtureClient {bool connect();bool close()const;};' +
                      ''.join('bool Client::' + fragment.removeprefix('bool ') for fragment in methods) + '}'))
    prelude = r'''
#include <cstdint>
namespace swoole {namespace http2 {struct Settings {uint32_t header_table_size=3,enable_push=0,max_concurrent_streams=0,init_window_size=4,max_frame_size=0,max_header_list_size=0;};}}
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <string>
#include <vector>
#include <functional>
#include <iostream>
#include <stdexcept>
#include <cstring>
#include <cerrno>
#define sw_unlikely(x) (x)
#define UNEXPECTED(x) (x)
#define ZEND_STRL(x) x,sizeof(x)-1
#define SW_ZSTR_KNOWN(x) (x)
#define SW_ZEND_STR_SETTING 1
#define SW_Z8_OBJ_P(x) (x)
#define Z_OBJCE_P(x) 0
#define SW_HTTP2_FRAME_HEADER_SIZE 9
#define SW_HTTP2_PRI_STRING "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"
struct SocketImpl;
using Socket=SocketImpl;
struct zend_object {SocketImpl *socket;};
struct zval {zend_object *object=nullptr;};
inline std::vector<std::string>effects;
inline unsigned fail_at,throw_at,calls;
inline std::function<void(const std::string&)>mutation;
inline bool effect(const std::string&text){effects.push_back(text);if(mutation)mutation(text);if(throw_at==effects.size())throw std::runtime_error("native effect");return ++calls!=fail_at;}
namespace network {struct Socket{static int convert_to_type(const std::string&host){effect("type:"+host);return 7;}};}
namespace Http2 {
using Settings=swoole::http2::Settings;
inline void init_settings(Settings *s){effect("defaults");s->header_table_size=12;s->init_window_size=21;}
inline long get_frame_length(){return 0;}
}
struct SocketImpl {unsigned id=1;int errCode=71;const char *errMsg="socket error";bool http2=false,open_length_check=false,closed=false,zero=false;unsigned close_count=0;struct Protocol {size_t package_length_size=0,package_length_offset=99,package_body_offset=99;long(*get_package_length)()=nullptr;}protocol;std::function<void(Socket*)>dtor;
 void set_dtor(std::function<void(Socket*)>f){effect("dtor:"+std::to_string(id));dtor=std::move(f);}
 void set_zero_copy(bool b){effect("zero:"+std::to_string(id));zero=b;}
 bool enable_ssl_encrypt(){return effect("ssl:"+std::to_string(id));}
 bool connect(const std::string&host,int port){return effect("connect:"+std::to_string(id)+":"+host+":"+std::to_string(port));}
 bool close(){++close_count;bool result=effect("close:"+std::to_string(id));closed=true;return result;}
};
inline zend_object created;
inline int swoole_http2_client_coro_ce;
inline zend_object *php_swoole_create_socket(int type){return effect("create:"+std::to_string(type))?&created:nullptr;}
inline void set_object(zval*z,zend_object*object){effect("object");z->object=object;}
#define ZVAL_OBJ(z,o) set_object(z,o)
inline SocketImpl *php_swoole_get_socket(zval*z){effect("get_socket");return z->object->socket;}
inline zval setting;
inline zval *sw_zend_read_property_ex(int,zval*,int,int){effect("setting");return &setting;}
inline void php_swoole_socket_set_error_properties(zval*,int code,const char*msg){effect("create_error:"+std::to_string(code)+":"+msg);}
inline int php_nghttp2_mem(){effect("hpack_mem");return 0;}
inline int nghttp2_hd_inflate_new2(void **slot,int){bool ok=effect("inflate");if(ok)*slot=&setting;return ok?0:-31;}
inline int nghttp2_hd_deflate_new2(void **slot,unsigned size,int){bool ok=effect("deflate:"+std::to_string(size));if(ok)*slot=&setting;return ok?0:-32;}
inline void zend_update_property(int,zval*,const char*,size_t,zval*){effect("publish_socket");}
inline void zend_update_property_bool(int,zval*,const char*,size_t,bool connected){effect("connected:"+std::to_string(connected));}
struct FixtureClient {
 mutable SocketImpl *socket_=nullptr;zval object,zsocket;zval*zobject=&object;std::string host="initial";int port=9501;bool open_ssl=false;unsigned stream_id=13,remote_window_size=17;Http2::Settings local_settings,remote_settings;void *inflater=nullptr,*deflater=nullptr;
 void socket_dtor(){effect("socket_dtor");socket_=nullptr;}
 void apply_setting(const zval*)const{effect("apply");}
 void update_error_properties(int code,const char*msg)const{effect("error:"+std::to_string(code)+":"+msg);}
 void io_error()const{effect("io:"+std::to_string(socket_->id)+":"+std::to_string(socket_->errCode));}
 void nghttp2_error(int code,const char*msg)const{effect("hpack_error:"+std::to_string(code)+":"+msg);}
 bool send(const char*,size_t length){return effect("send:"+std::to_string(length));}
 bool send_setting(){return effect("settings");}
};
'''
    driver = r'''
struct Result {std::vector<std::string>events;bool returned=false;std::string exception;unsigned stream=0,window=0,local=0,remote=0,live=0,closes=0;bool inflate=false,deflate=false;std::vector<size_t>protocol;};
template<class Client>Result execute(bool closing,bool present,bool ssl,unsigned failure,unsigned change,unsigned thrown){
 effects.clear();calls=0;fail_at=failure;throw_at=thrown;errno=ENOENT;Client client;SocketImpl first,second;second.id=2;created.socket=&first;client.open_ssl=ssl;client.socket_=present?&first:nullptr;
 mutation=[&](const std::string&event){
 if((change&1)&&event.rfind("zero:",0)==0)client.open_ssl=!client.open_ssl;
 if((change&2)&&(event=="apply"||event=="inflate")){client.local_settings.header_table_size+=17;client.host="changed";client.port=44;}
 if((change&4)&&(event=="connected:0"||event.rfind("close:",0)==0))client.socket_=nullptr;
 if((change&8)&&(event=="inflate"||event=="send:24"))client.socket_=&second;
 if((change&16)&&event=="setting"){client.remote_settings.init_window_size=99;client.stream_id=7;}
 };
 Result r;try{r.returned=closing?client.close():client.connect();}catch(const std::runtime_error&e){r.exception=e.what();}
 r.events=effects;r.stream=client.stream_id;r.window=client.remote_window_size;r.local=client.local_settings.header_table_size;r.remote=client.remote_settings.init_window_size;r.live=client.socket_?client.socket_->id:0;r.closes=first.close_count+second.close_count;r.inflate=client.inflater!=nullptr;r.deflate=client.deflater!=nullptr;
 for(auto*s:{&first,&second}){r.protocol.push_back(s->http2);r.protocol.push_back(s->open_length_check);r.protocol.push_back(s->protocol.package_length_size);r.protocol.push_back(s->protocol.package_length_offset);r.protocol.push_back(s->protocol.package_body_offset);r.protocol.push_back(s->protocol.get_package_length!=nullptr);r.protocol.push_back(s->zero);r.protocol.push_back(bool(s->dtor));}
 mutation={};return r;
}
static bool same(const Result&a,const Result&b){return a.events==b.events&&a.returned==b.returned&&a.exception==b.exception&&a.stream==b.stream&&a.window==b.window&&a.local==b.local&&a.remote==b.remote&&a.live==b.live&&a.closes==b.closes&&a.inflate==b.inflate&&a.deflate==b.deflate&&a.protocol==b.protocol;}
int main(){size_t count=0;
for(bool closing:{false,true})for(bool present:{false,true})for(bool ssl:{false,true})for(unsigned failure=0;failure<33;++failure)for(unsigned change=0;change<32;++change)for(unsigned thrown=0;thrown<33;++thrown){
 auto a=execute<native::Client>(closing,present,ssl,failure,change,thrown);auto b=execute<rust::Client>(closing,present,ssl,failure,change,thrown);if(!same(a,b)){std::cerr<<"connection mismatch closing="<<closing<<" present="<<present<<" ssl="<<ssl<<" failure="<<failure<<" mutation="<<change<<" thrown="<<thrown<<"\n";for(auto&e:a.events)std::cerr<<"native "<<e<<"\n";for(auto&e:b.events)std::cerr<<"rust "<<e<<"\n";return 1;}++count;
}
std::cout<<"{\"passed\":true,\"connection_cases\":"<<count<<"}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    report['commands'], result = sides.run(work, 'connection', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include)
    report.update(result or {})
    if gate.digest(header) != core['module_header_sha256']:
        report['passed'] = False
        report['error'] = 'Module header changed during checks'
    report['native_sanitizers'] = ['address', 'undefined except alignment']
    report['rust_sanitized'] = False
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'report': str(args.output)}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
