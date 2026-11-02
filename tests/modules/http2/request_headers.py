#!/usr/bin/env python3
"""Actual client request header builder and native HeaderSet allocation/publication bodies."""
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
    if gate.digest(archive) != core['archive_sha256']:
        raise ValueError('Archive changed')
    name = 'ext-src/swoole_http2_client_coro.cc'
    selected = artifacts / 'overlay' / name
    if gate.digest(selected) != core['patched_sources'][name]:
        raise ValueError('Overlay changed')
    if gate.digest(gate.ROOT / 'native/http2/rust_http2.h') != core['module_header_sha256']:
        raise ValueError('Module header changed')
    original = subprocess.check_output(['git', 'show', f'{gate.PIN}:{name}'], cwd=args.source, text=True)
    php_header = subprocess.check_output(['git', 'show', f'{gate.PIN}:ext-src/php_swoole_http.h'], cwd=args.source, text=True)
    header_set = 'class HeaderSet {' + body(php_header, 'class HeaderSet') + '};'
    report = {'passed': False, 'scope': __doc__, 'upstream_commit': gate.PIN,
              'commands': [],
              'gaps': ['Real PHP conversions/refcounts/Zend longjmp, HPACK/URL encoding and allocator implementations',
                       'Original duplicate-host HeaderSet overwrites and exceptional allocation remnants are preserved, not repaired',
                       'Invalidated raw-pointer lifetimes and original undefined HeaderSet bounds are excluded',
                       'Rust sanitizer instrumentation and performance']}
    pieces = []
    for variant, text in [('native', original), ('rust', select_overlay(selected.read_text()))]:
        fragment = body(text, 'ssize_t Client::build_header(')
        pieces.append((variant, 'namespace ' + variant + ' {struct Client;static Client*http2_client_coro_get_client(const zval*);struct Client: FixtureClient {ssize_t build_header(const zval*zobj,zval*zrequest,char*buffer){' + fragment + '}};static Client*http2_client_coro_get_client(const zval*){event("get-client");return static_cast<Client*>(active_client);}}'))
    prelude = r'''
#include "swoole_http2.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <string>
#include <vector>
#include <map>
#include <memory>
#include <functional>
#include <iostream>
#include <stdexcept>
#include <cstdlib>
#include <cstdio>
#include <clocale>
using namespace swoole;
namespace Http2=swoole::http2;
#define ZEND_STRL(x) x,sizeof(x)-1
#define HTTP2_CLIENT_HOST_HEADER_INDEX 3
#define NGHTTP2_NV_FLAG_NONE 0
#define NGHTTP2_NV_FLAG_NO_COPY_NAME 1
#define NGHTTP2_NV_FLAG_NO_COPY_VALUE 2
#define E_WARNING 2
struct nghttp2_nv {unsigned char*name=nullptr,*value=nullptr;size_t namelen=0,valuelen=0;unsigned char flags=0;};
inline std::vector<std::string> effects;
inline std::map<void*,unsigned> allocations;
inline unsigned serial,throw_at,mutation,encoding_fail;
inline std::function<void(const std::string&)> mutate;
inline void event(std::string text,bool may_throw=true){effects.push_back(text);if(mutate)mutate(text);if(may_throw&&throw_at==effects.size())throw std::runtime_error("native effect");}
inline void*allocate(size_t bytes,bool zero){event(std::string(zero?"calloc:":"malloc:")+std::to_string(bytes));void*p=zero?std::calloc(1,bytes?bytes:1):std::malloc(bytes?bytes:1);if(!p)std::abort();allocations[p]=serial++;return p;}
inline void*ecalloc(size_t n,size_t size){return allocate(n*size,true);}
inline void*emalloc(size_t size){return allocate(size,false);}
inline void efree(void*p){if(!p)return;auto it=allocations.find(p);if(it==allocations.end())std::abort();event("free:"+std::to_string(it->second),false);allocations.erase(it);std::free(p);}
inline char*zend_str_tolower_dup(const char*p,size_t n){event("lower:"+std::string(p,n));auto out=(char*)emalloc(n+1);for(size_t i=0;i<n;++i)out[i]=(p[i]>='A'&&p[i]<='Z')?p[i]+32:p[i];out[n]=0;return out;}
inline void php_swoole_fatal_error(int,const char*,int,const char*){throw std::runtime_error("native header bound");}
#undef swoole_trace_log
#define swoole_trace_log(...) ((void)0)
'''
    prelude += 'namespace swoole {namespace http2 {' + header_set + '}}\nusing Http2::HeaderSet;\n'
    prelude += r'''
struct zend_string {std::string bytes;};
struct zval;
struct Entry {zend_string*key;zval*value;};
struct zval {int kind=1;std::string bytes;std::vector<Entry> entries;};
#define IS_STRING 1
#define Z_TYPE_P(z) ((z)->kind)
#define Z_STRVAL_P(z) ((z)->bytes.data())
#define Z_STRLEN_P(z) ((z)->bytes.size())
#define ZVAL_IS_ARRAY(z) ((z)->kind==2)
#define ZVAL_IS_NULL(z) ((z)->kind==0)
#define Z_ARRVAL_P(z) (z)
#define ZSTR_VAL(z) ((z)->bytes.data())
#define ZSTR_LEN(z) ((z)->bytes.size())
#define ZEND_HASH_FOREACH_STR_KEY_VAL(table,out_key,out_value) for(auto&entry:(table)->entries) if(((out_key=entry.key),(out_value=entry.value),true))
#define UNEXPECTED(x) (x)
#define ZEND_HASH_FOREACH_END()
#define SW_ZSTR_KNOWN(x) (x)
#define SW_ZEND_STR_METHOD 0
#define SW_ZEND_STR_PATH 1
#define SW_ZEND_STR_HEADERS 2
#define SW_ZEND_STR_COOKIES 3
inline int swoole_http2_request_ce;
inline zval properties[4];
inline zval*sw_zend_read_property_ex(int,zval*,int field,int){event("property:"+std::to_string(field));return &properties[field];}
inline size_t php_swoole_array_length_safe(zval*z){event("array-length");return ZVAL_IS_ARRAY(z)?z->entries.size():0;}
namespace zend {struct String {zval*value;explicit String(zval*z):value(z){event("convert:"+z->bytes);}~String(){event("string-drop",false);}char*val(){return value->bytes.data();}size_t len(){return value->bytes.size();}};}
namespace std_string {inline std::string format(const char*pattern,const char*host,int port){event("format");char buffer[256];int n=std::snprintf(buffer,sizeof(buffer),pattern,host,port);if(n<0||n>=int(sizeof(buffer)))std::abort();return std::string(buffer,n);}}
struct Scratch {char storage[256]{};char*str=storage;size_t length=0;void clear(){event("clear");length=0;}void append(const char*p,size_t n){event("append:"+std::string(p,n));if(length+n>sizeof(storage))std::abort();std::memcpy(storage+length,p,n);length+=n;}};
inline Scratch scratch;
#define sw_tg_buffer fixture_buffer
inline Scratch*fixture_buffer(){event("buffer");return &scratch;}
inline char*php_swoole_url_encode(const char*p,size_t n,size_t*out){event("encode:"+std::string(p,n));if(encoding_fail)return nullptr;*out=n+2;auto bytes=(char*)emalloc(*out);bytes[0]='<';std::memcpy(bytes+1,p,n);bytes[n+1]='>';return bytes;}
inline unsigned deflate_error;
inline size_t nghttp2_hd_deflate_bound(void*,nghttp2_nv*values,size_t n){event("bound:"+std::to_string(n));for(size_t i=0;i<n;++i)event("nv:"+std::to_string(i)+":"+std::string((char*)values[i].name,values[i].namelen)+":"+std::string((char*)values[i].value,values[i].valuelen));return 128;}
inline ssize_t nghttp2_hd_deflate_hd(void*,unsigned char*buffer,size_t bound,nghttp2_nv*,size_t n){event("deflate:"+std::to_string(bound)+":"+std::to_string(n));if(deflate_error)return -1;std::memset(buffer,'C',7);return 7;}
struct FixtureClient {using String=Scratch;std::string host="example.test";int port=80;bool open_ssl=false;void*deflater=(void*)1;void nghttp2_error(int rc,const char*what){event("hpack-error:"+std::to_string(rc)+":"+what);}};
inline FixtureClient*active_client;
'''
    driver = r'''
struct Result {std::vector<std::string> events;ssize_t returned=-2;unsigned remaining=0;int port=0;bool ssl=false;std::string exception,output;};
template<class C>Result execute(unsigned method,unsigned path,bool tls,int port,unsigned headers,unsigned cookies,unsigned failure,unsigned change,unsigned throw_index){
 effects.clear();serial=0;throw_at=throw_index;mutation=change;encoding_fail=failure&1;deflate_error=failure&2;C client;active_client=&client;client.open_ssl=tls;client.port=port;
 properties[0]=zval{method==0?0:1,method==2?"POST":"",{}};properties[1]=zval{path==0?0:1,path==2?"/p?x=y":"",{}};properties[2]=zval{2,"",{}};properties[3]=zval{2,"",{}};
 zend_string host{"Host"},other{"x-extra"},pseudo{":scheme"},cookie{"session"};zval host_value{1,"custom.test",{}},other_value{1,std::string("a\0b",3),{}},null_value{0,"",{}};
 if(headers==1)properties[2].entries={{&host,&host_value}};
 if(headers==2)properties[2].entries={{&other,&other_value},{&pseudo,&host_value}};
 if(headers==3)properties[2].entries={{nullptr,&host_value},{&other,&null_value}};
 if(headers==4)properties[2].entries={{&host,&host_value},{&host,&other_value}};
 if(cookies==1)properties[3].entries={{&cookie,&other_value}};
 if(cookies==2)properties[3].entries={{nullptr,&host_value},{&cookie,&null_value}};
 bool altered_path=false;mutate=[&](const std::string&what){if((change&1)&&what.rfind("lower:",0)==0&&!altered_path){properties[1].kind=0;altered_path=true;}if((change&2)&&what.rfind("convert:",0)==0){client.open_ssl=!client.open_ssl;client.port=443;}};
 zval request;char output[256]{};Result r;try{r.returned=client.build_header(&request,&request,output);}catch(const std::runtime_error&e){r.exception=e.what();}
 r.events=effects;r.remaining=allocations.size();r.port=client.port;r.ssl=client.open_ssl;if(r.returned>0)r.output.assign(output,r.returned);mutate={};for(auto [pointer,id]:allocations)std::free(pointer);allocations.clear();return r;
}
static bool same(const Result&a,const Result&b){return a.events==b.events&&a.returned==b.returned&&a.remaining==b.remaining&&a.port==b.port&&a.ssl==b.ssl&&a.exception==b.exception&&a.output==b.output;}
int main(){size_t count=0;
for(unsigned method:{0u,1u,2u})for(unsigned path:{0u,1u,2u})for(bool tls:{false,true})for(int port:{-1,0,80,443,65535})for(unsigned headers=0;headers<5;++headers)for(unsigned cookies=0;cookies<3;++cookies)for(unsigned failure=0;failure<4;++failure)for(unsigned change=0;change<4;++change){
 auto a=execute<native::Client>(method,path,tls,port,headers,cookies,failure,change,0);auto b=execute<rust::Client>(method,path,tls,port,headers,cookies,failure,change,0);if(!same(a,b)){std::cerr<<"request headers mismatch method="<<method<<" path="<<path<<" tls="<<tls<<" port="<<port<<" headers="<<headers<<" cookies="<<cookies<<" failure="<<failure<<" mutation="<<change<<"\n";return 1;}++count;
}
for(unsigned headers=0;headers<5;++headers)for(unsigned change=0;change<4;++change)for(unsigned throw_index=1;throw_index<50;++throw_index){
 auto a=execute<native::Client>(2,2,false,80,headers,1,0,change,throw_index);auto b=execute<rust::Client>(2,2,false,80,headers,1,0,change,throw_index);if(!same(a,b)){std::cerr<<"request headers effect-prefix mismatch\n";return 2;}++count;
}
std::cout<<"{\"passed\":true,\"request_header_cases\":"<<count<<"}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    report['commands'], result = sides.run(work, 'request_headers', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include)
    report.update(result or {})
    report['native_sanitizers'] = ['address', 'undefined except alignment']
    report['rust_sanitized'] = False
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'report': str(args.output)}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
