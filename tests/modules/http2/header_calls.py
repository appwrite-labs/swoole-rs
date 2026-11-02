#!/usr/bin/env python3
"""Actual client header parser with scripted HPACK/Zend/zlib effects, plus locale policy checks."""
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
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source',type=Path,required=True)
    parser.add_argument('--core-report',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--include',action='append',default=[])
    args=parser.parse_args();core=json.loads(args.core_report.read_text())
    if not core['passed']:raise ValueError('Completed core receipt required')
    artifacts=args.core_report.parent/(args.core_report.stem+'-artifacts');archive=Path(core.get('archive',artifacts/'libhttp2.a'))
    if gate.digest(archive)!=core['archive_sha256']:raise ValueError('Archive identity mismatch')
    filename='ext-src/swoole_http2_client_coro.cc';selected=artifacts/'overlay'/filename
    if gate.digest(selected)!=core['patched_sources'][filename]:raise ValueError('Overlay identity mismatch')
    original=subprocess.check_output(['git','show',f'{gate.PIN}:{filename}'],cwd=args.source,text=True)
    marker='int Client::parse_header(';pieces=[]
    report={'passed':False,'scope':__doc__,'upstream_commit':gate.PIN,'commands':[],
            'gaps':['actual PHP/HPACK/zlib runtime','server PHP parser effects','unselected locales','performance','Rust instrumentation']}
    for variant,text in [('native',original),('rust',select_overlay(selected.read_text()))]:
        fragment=body(text,marker)
        pieces.append((variant, 'namespace '+variant+' {struct Client: FixtureClient {int parse_header(Stream *stream,int flags,char *in,size_t inlen) const {'+fragment+'}};}\n'))
    prelude=r'''
#include "swoole_http2.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <array>
#include <string>
#include <vector>
#include <iostream>
#include <cstdarg>
#include <clocale>
#include <stdexcept>
#include <cctype>
#include <strings.h>
using namespace swoole;
namespace Http2=swoole::http2;
#undef SW_HAVE_ZLIB
#if TEST_ZLIB
#define SW_HAVE_ZLIB
#endif
#define ZEND_STRL(s) s,sizeof(s)-1
#define NGHTTP2_HD_INFLATE_EMIT 2
#define NGHTTP2_HD_INFLATE_FINAL 1
#define Z_OK 0
#define MAX_WBITS 15
struct zval {int id=0;};
struct FakeZ {void *(*zalloc)(void*,unsigned,unsigned);void (*zfree)(void*,void*);};
struct Stream {zval zresponse;int gzip=0;FakeZ gzip_stream{};void *gzip_buffer=nullptr;};
struct nghttp2_nv {uint8_t *name,*value;size_t namelen,valuelen;};
inline std::vector<std::string> effects;
inline int zresult,decode_error,emit_header,throw_effect,completion_mode,decode_calls;
inline std::string field,value;
inline int swoole_http2_response_ce;
inline void emit(const char *fmt,...){char b[512];va_list v;va_start(v,fmt);vsnprintf(b,sizeof(b),fmt,v);va_end(v);effects.push_back(b);}
#undef swoole_trace_log
#define swoole_trace_log(flags,...) emit(__VA_ARGS__)
#undef swoole_warning
#define swoole_warning(...) emit(__VA_ARGS__)
inline zval properties[3]={{1},{2},{3}};
inline zval *sw_zend_read_and_convert_property_array(int,zval*,const char *p,size_t n,int){effects.push_back("property:"+std::string(p,n));if(throw_effect==1)throw std::runtime_error("native property");return &properties[p[0]=='h'?0:p[0]=='c'?1:2];}
namespace zend {inline void object_set(zval*,const char*p,size_t n,long v){effects.push_back("set:"+std::string(p,n)+":"+std::to_string(v));if(throw_effect==2)throw std::runtime_error("native set");}}
inline void add_assoc_stringl_ex(zval*z,const char*k,size_t kn,const char*v,size_t vn){effects.push_back("assoc:"+std::to_string(z->id)+":"+std::string(k,kn)+":"+std::string(v,vn));if(throw_effect==3)throw std::runtime_error("native assoc");}
inline void php_swoole_http_parse_set_cookies(const char*v,size_t n,zval*,zval*){effects.push_back("cookie:"+std::string(v,n));if(throw_effect==4)throw std::runtime_error("native cookie");}
inline ssize_t nghttp2_hd_inflate_hd(void*,nghttp2_nv*nv,int*flags,uint8_t*,size_t,int){effects.push_back("inflate");if(decode_error)return -1;*nv={(uint8_t*)field.data(),(uint8_t*)value.data(),field.size(),value.size()};*flags=NGHTTP2_HD_INFLATE_FINAL|(emit_header?NGHTTP2_HD_INFLATE_EMIT:0);if(completion_mode&&decode_calls++==0){*flags=emit_header?NGHTTP2_HD_INFLATE_EMIT:0;return completion_mode==1?1:0;}return 1;}
inline void nghttp2_hd_inflate_end_headers(void*){effects.push_back("end-headers");}
inline void *php_zlib_alloc(void*,unsigned,unsigned){return nullptr;}
inline void php_zlib_free(void*,void*){}
#define make_string fixture_make_string
inline void *fixture_make_string(size_t size){effects.push_back("buffer:"+std::to_string(size));return (void*)1;}
inline int inflateInit2(FakeZ*,int window){effects.push_back("zlib:"+std::to_string(window));return zresult;}
struct FixtureClient {void *inflater=nullptr;void nghttp2_error(int rc,const char *what) const {effects.push_back("hpack-error:"+std::to_string(rc)+":"+what);}};
'''
    driver=r'''
// ASan's strcasecmp interceptors fold ASCII only; glibc folds through the current
// locale, which upstream's comparisons and the Rust side's fold both rely on (the
// tr_TR case). Define them once, in the driver, with glibc's semantics so sanitized runs agree.
#ifndef __THROW
#define __THROW
#endif
extern "C" int strncasecmp(const char *a, const char *b, size_t n) __THROW {
    for (; n; --n, ++a, ++b) {
        int d = std::tolower((unsigned char) *a) - std::tolower((unsigned char) *b);
        if (d || !*a) return d;
    }
    return 0;
}
extern "C" int strcasecmp(const char *a, const char *b) __THROW { return strncasecmp(a, b, (size_t) -1); }
static unsigned expected(unsigned role,const std::string&s){const char*p=s.data();size_t n=s.size();
 if(role==0){if(s.size()&&s[0]==':'){p++;n--;if(SW_STRCASEEQ(p,n,"method"))return 1;if(SW_STRCASEEQ(p,n,"path"))return 2;if(SW_STRCASEEQ(p,n,"authority"))return 3;}
 else{if(SW_STRCASEEQ(p,n,"content-type"))return 4;if(SW_STRCASEEQ(p,n,"cookie"))return 5;if(SW_STRCASEEQ(p,n,"content-length"))return 6;if(SW_STRCASEEQ(p,n,"accept-encoding"))return 7;}}
 if(role==1){if(s.size()&&s[0]==':'){if(SW_STRCASEEQ(p+1,n-1,"status"))return 8;}else{if(SW_STRCASEEQ(p,n,"content-encoding"))return 9;if(SW_STRCASEEQ(p,n,"set-cookie"))return 10;}}
 if(role==2){if(SW_STRCASEEQ(p,n,"server"))return 11;if(SW_STRCASEEQ(p,n,"content-length"))return 6;if(SW_STRCASEEQ(p,n,"date"))return 12;if(SW_STRCASEEQ(p,n,"content-type"))return 4;}
 if(role==3&&SW_STRCASEEQ(p,n,"host"))return 13;return 0;}
int main(){size_t count=0;std::vector<std::string> names={"",":status",":STATUS",":method",":path",":authority","content-type","CONTENT-TYPE","content-length","accept-encoding","content-encoding","set-cookie","Set-Cookie","server","date","HOST","cookie",std::string(200,'a')};
for(unsigned c=0;c<256;++c){names.push_back(std::string(1,char(c))+"status");names.push_back("content-"+std::string(1,char(c))+"ype");names.push_back("set-cookie"+std::string(1,char(c)));}
std::vector<std::string> values={"","200","00301","application/x-www-form-urlencoded","APPLICATION/X-WWW-FORM-URLENCODED; charset=x","multipart/form-data; boundary=a","MULTIPART/FORM-DATA","gzip","GZip;stuff","gzipgarbage","a=b;c=d",std::string("gzip\0tail",9),std::string(40,'a')};
std::vector<std::string> locales,unavailable_locales;for(const char *locale:{"C","C.UTF-8","en_US.UTF-8","tr_TR.ISO8859-9"}){if(!setlocale(LC_CTYPE,locale)){unavailable_locales.push_back(locale);continue;}locales.push_back(locale);
 for(auto&name:names)for(unsigned role=0;role<4;++role){if(expected(role,name)!=Http2::rust_header_kind((SwRsHttp2HeaderRole)role,name.data(),name.size())){std::cerr<<"header kind mismatch\n";return 1;}++count;}
 for(auto&v:values){unsigned want=SW_STR_ISTARTS_WITH(v.data(),v.size(),"application/x-www-form-urlencoded")?1:SW_STR_ISTARTS_WITH(v.data(),v.size(),"multipart/form-data")?2:SW_STR_ISTARTS_WITH(v.data(),v.size(),"gzip")?3:0;
  if(want!=Http2::rust_media_kind(v.data(),v.size()))return 2;++count;}
 for(auto&name:names)for(auto&v:values)for(int error:{0,1})for(int zr:{0,-3})for(int emit_it:{0,1}){
  field=name;value=v;decode_error=error;zresult=zr;emit_header=emit_it;throw_effect=0;
  std::vector<std::string> reference;int code=0;Stream reference_stream;
  for(bool rust:{false,true}){effects.clear();Stream stream;char input[16]={};int rc=rust?::rust::Client{}.parse_header(&stream,SW_HTTP2_FLAG_PRIORITY,input,sizeof(input)) : ::native::Client{}.parse_header(&stream,SW_HTTP2_FLAG_PRIORITY,input,sizeof(input));
   if(!rust){reference=effects;code=rc;reference_stream=stream;}
   else if(reference!=effects||code!=rc||reference_stream.gzip!=stream.gzip||reference_stream.gzip_buffer!=stream.gzip_buffer){std::cerr<<"parser mismatch locale="<<locale<<" name="<<name<<" value="<<v<<"\n";return 3;}}
  ++count;
 }
}
// Native throws occur outside Rust frames; compare effect prefixes and immediate propagation.
for(auto&name:std::vector<std::string>{":status","set-cookie","content-encoding"})for(int stage:{1,2,3,4}){
 field=name;value=name==":status"?"200":"gzip";decode_error=0;emit_header=1;zresult=0;throw_effect=stage;std::vector<std::string> reference;std::string message;
 for(bool rust:{false,true}){effects.clear();Stream stream;char input[16]={};std::string raised;try{if(rust)::rust::Client{}.parse_header(&stream,0,input,sizeof(input));else ::native::Client{}.parse_header(&stream,0,input,sizeof(input));}catch(const std::runtime_error&e){raised=e.what();}
 if(!rust){reference=effects;message=raised;}else if(reference!=effects||message!=raised)return 4;}++count;
}
// Non-final exhaustion versus another decode: actual role-specific loop decisions.
for(int mode:{1,2})for(int emission:{0,1})for(size_t input_len:{size_t(1),size_t(2)}){
 completion_mode=mode;field="x-extra";value="value";decode_error=throw_effect=0;emit_header=emission;zresult=0;std::vector<std::string> reference;int code=0;
 for(bool rust:{false,true}){effects.clear();decode_calls=0;Stream stream;char input[2]={};int rc=rust?::rust::Client{}.parse_header(&stream,0,input,input_len) : ::native::Client{}.parse_header(&stream,0,input,input_len);
 if(!rust){reference=effects;code=rc;}else if(reference!=effects||code!=rc){std::cerr<<"client completion mismatch\n";return 5;}}
 ++count;
}
std::cout<<"{\"passed\":true,\"header_cases\":"<<count<<",\"zlib_enabled\":"<<TEST_ZLIB<<",\"locales\":[";for(size_t i=0;i<locales.size();++i){if(i)std::cout<<",";std::cout<<"\""<<locales[i]<<"\"";}std::cout<<"],\"unavailable_locales\":[";for(size_t i=0;i<unavailable_locales.size();++i){if(i)std::cout<<",";std::cout<<"\""<<unavailable_locales[i]<<"\"";}std::cout<<"]}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    commands, report['variants'] = sides.variants(work, 'headers', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include, define='TEST_ZLIB')
    report['commands'] += commands
    report['passed']=len(report['variants'])==2 and all(r['passed'] for r in report['variants']);report['native_sanitizers']=['address','undefined except alignment'];report['rust_sanitized']=False
    args.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({'passed':report['passed'],'report':str(args.output)}));return 0 if report['passed'] else 1

if __name__=='__main__':raise SystemExit(main())
