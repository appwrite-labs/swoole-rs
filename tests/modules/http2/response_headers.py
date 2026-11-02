#!/usr/bin/env python3
"""Actual outgoing server header lambda with native conversion/warning/capture effects."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
sys.path.insert(0,str(Path(__file__).resolve().parent))
import gate
import sides
from settings_calls import body,select_overlay


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--core-report',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--include',action='append',default=[]);a=p.parse_args();core=json.loads(a.core_report.read_text())
    if not core['passed']:raise ValueError('Completed core receipt required')
    artifacts=a.core_report.parent/(a.core_report.stem+'-artifacts');archive=Path(core.get('archive',artifacts/'libhttp2.a'))
    if gate.digest(archive)!=core['archive_sha256']:raise ValueError('Archive changed')
    name='ext-src/swoole_http2_server.cc';selected=artifacts/'overlay'/name
    if gate.digest(selected)!=core['patched_sources'][name]:raise ValueError('Overlay changed')
    original=subprocess.check_output(['git','show',f'{gate.PIN}:{name}'],cwd=a.source,text=True)
    php_header=subprocess.check_output(['git','show',f'{gate.PIN}:ext-src/php_swoole_http.h'],cwd=a.source,text=True)
    report={'passed':False,'scope':__doc__,'upstream_commit':gate.PIN,'commands':[],
            'gaps':['actual PHP/HPACK/allocator/runtime and Zend longjmp bailout','complete header builder/refcount lifetime','native rtrim signed-char undefined inputs','unselected locales/platforms','Rust sanitizer instrumentation','performance']}
    pieces=[]
    for variant,text in [('native',original),('rust',select_overlay(selected.read_text()))]:
        fragment=body(text,'[ctx, &content_type]')
        pieces.append((variant, 'namespace '+variant+' {void add(Ctx *ctx,Http2::HeaderSet &headers,const char *key,size_t l_key,zval *value,uint32_t &header_flags,zend_string *&content_type){'+fragment+'}}\n'))
    warning_body=body(php_header,'static sw_inline bool swoole_http_has_crlf(')
    flags=body(php_header,'enum swHttpHeaderFlag')
    prelude=r'''
#include "swoole_http2.h"
#include "swoole_util.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <string>
#include <vector>
#include <memory>
#include <functional>
#include <iostream>
#include <stdexcept>
#include <cstdarg>
#include <clocale>
using namespace swoole;
namespace Http2=swoole::http2;
#undef SW_HAVE_COMPRESSION
#if TEST_COMPRESSION
#define SW_HAVE_COMPRESSION
#endif
#define E_WARNING 2
inline std::vector<std::string> effects;
inline unsigned throwing;
inline std::function<void()> conversion,capture;
inline void php_swoole_error(int,const char *what){effects.push_back("warning:"+std::string(what));if(throwing==3)throw std::runtime_error("native warning");}
struct zend_string {std::string bytes;size_t length;};
struct zval {bool null=false;zend_string str;};
#define ZVAL_IS_NULL(v) ((v)->null)
inline std::vector<std::unique_ptr<zend_string>> strings;
inline zend_string *zval_get_string(zval*v){effects.push_back("capture:"+v->str.bytes.substr(0,v->str.length));if(capture)capture();if(throwing==4)throw std::runtime_error("native capture");strings.push_back(std::make_unique<zend_string>(v->str));return strings.back().get();}
namespace zend {struct String {zval *value;
 String(zval*v):value(v){effects.push_back("convert");if(conversion)conversion();if(throwing==1)throw std::runtime_error("native conversion");}
 ~String(){effects.push_back("string-release");}
 void rtrim() const {effects.push_back("trim");value->str.length=swoole::rtrim(value->str.bytes.data(),value->str.length);if(throwing==2)throw std::runtime_error("native trim");}
 char *val() const {return value->str.bytes.data();}size_t len() const {return value->str.length;}
};}
struct Ctx {bool accept_compression;void *compression_types;};
namespace swoole {namespace http2 {struct HeaderSet {void add(const char*k,size_t kn,const char*v,size_t vn){effects.push_back("add:"+std::string(k,kn)+":"+std::string(v,vn));if(throwing==5)throw std::runtime_error("native add");}};}}
'''
    prelude+='enum swHttpHeaderFlag {'+flags+'};\ninline bool swoole_http_has_crlf(const char *value,size_t length){'+warning_body+'}\n'
    driver=r'''
struct Result {std::vector<std::string> events;std::string exception,captured,remaining;uint32_t flags;bool accept,types;};
static Result execute(bool rust,const std::string &key,const std::string &bytes,bool null,bool accept,bool types,uint32_t flags,unsigned scenario,unsigned fault){
 strings.clear();effects.clear();throwing=fault;Ctx ctx{accept,types?(void*)1:nullptr};zval value{null,{bytes,bytes.size()}};zend_string *content_type=nullptr;Http2::HeaderSet headers;
 conversion=[&](){if(scenario&1)ctx.accept_compression=!ctx.accept_compression;if(scenario&2)ctx.compression_types=ctx.compression_types?nullptr:(void*)1;};capture=[&](){if(scenario&4)ctx.accept_compression=!ctx.accept_compression;};
 std::string raised;try{if(rust)::rust::add(&ctx,headers,key.data(),key.size(),&value,flags,content_type);else ::native::add(&ctx,headers,key.data(),key.size(),&value,flags,content_type);}catch(const std::runtime_error&e){raised=e.what();}
 return {effects,raised,content_type?content_type->bytes.substr(0,content_type->length):"",value.str.bytes.substr(0,value.str.length),flags,ctx.accept_compression,bool(ctx.compression_types)};
}
int main(){size_t count=0;std::vector<std::string> keys={"","server","SERVER","content-length","CONTENT-LENGTH","date","DATE","content-type","CONTENT-TYPE","unknown",std::string("date\0x",6)};
for(unsigned byte=0;byte<256;++byte)keys.push_back("content-"+std::string(1,char(byte))+"ype");
std::vector<std::string> values={"","a","a ","a\r\n","a\ninside","a\rinside",std::string("a\0inside",8)," \t\r\n"};for(unsigned byte=0;byte<128;++byte)values.push_back("a"+std::string(1,char(byte)));
std::vector<std::string> locales,missing;for(const char *locale:{"C","C.UTF-8","en_US.UTF-8","tr_TR.ISO8859-9"}){if(!setlocale(LC_CTYPE,locale)){missing.push_back(locale);continue;}locales.push_back(locale);
for(auto&key:keys)for(auto&value:values)for(bool null:{false,true})for(bool accept:{false,true})for(bool types:{false,true}){
 unsigned scenario=(key.size()+value.size())%8;unsigned fault=(key.size()*value.size())%6;uint32_t flags=key.size()%2?UINT32_MAX:0;
 auto a=execute(false,key,value,null,accept,types,flags,scenario,fault);auto b=execute(true,key,value,null,accept,types,flags,scenario,fault);
 if(a.events!=b.events||a.exception!=b.exception||a.captured!=b.captured||a.remaining!=b.remaining||a.flags!=b.flags||a.accept!=b.accept||a.types!=b.types){std::cerr<<"outgoing header mismatch locale="<<locale<<" key="<<key<<"\n";return 1;}++count;
}}
for(uint32_t flags=0;flags<1024;++flags){uint32_t expected=0;if(!(flags&HTTP_HEADER_SERVER))expected|=HTTP_HEADER_SERVER;if(!(flags&HTTP_HEADER_DATE))expected|=HTTP_HEADER_DATE;if(!(flags&HTTP_HEADER_CONTENT_TYPE))expected|=HTTP_HEADER_CONTENT_TYPE;if(sw_rs_http2_header_defaults(flags)!=expected)return 2;++count;}
for(int64_t parsed:{INT64_MIN,int64_t(-1),int64_t(0),int64_t(1),int64_t(UINT32_MAX),INT64_MAX})for(uint32_t maximum:{0u,1u,UINT32_MAX})for(bool complete:{false,true}){if(bool(sw_rs_http2_content_length_rejected(complete,parsed,maximum))!=(!complete||parsed>maximum))return 3;++count;}
std::cout<<"{\"passed\":true,\"response_header_cases\":"<<count<<",\"compression_enabled\":"<<TEST_COMPRESSION<<",\"locales\":[";for(size_t i=0;i<locales.size();++i){if(i)std::cout<<",";std::cout<<"\""<<locales[i]<<"\"";}std::cout<<"],\"unavailable_locales\":[";for(size_t i=0;i<missing.size();++i){if(i)std::cout<<",";std::cout<<"\""<<missing[i]<<"\"";}std::cout<<"]}\n";
}
'''
    work = a.output.parent / (a.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    commands, report['variants'] = sides.variants(work, 'response', prelude, pieces, driver, source=a.source, archive=archive, includes=a.include, define='TEST_COMPRESSION')
    report['commands'] += commands
    report['passed']=len(report['variants'])==2 and all(r['passed'] for r in report['variants']);report['native_sanitizers']=['address','undefined except alignment'];report['rust_sanitized']=False;a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({'passed':report['passed'],'report':str(a.output)}));return 0 if report['passed'] else 1

if __name__=='__main__':raise SystemExit(main())
