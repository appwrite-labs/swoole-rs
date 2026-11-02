#!/usr/bin/env python3
"""Actual server header parser with scripted HPACK/Zend/path/multipart/compression effects."""
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
    filename='ext-src/swoole_http2_server.cc';selected=artifacts/'overlay'/filename
    if gate.digest(selected)!=core['patched_sources'][filename]:raise ValueError('Overlay identity mismatch')
    original=subprocess.check_output(['git','show',f'{gate.PIN}:{filename}'],cwd=args.source,text=True)
    marker='static int http2_server_parse_header(';pieces=[]
    report={'passed':False,'scope':__doc__,'upstream_commit':gate.PIN,'commands':[],
            'gaps':['actual PHP/HPACK/zlib runtime','real server PHP/GC/property/cookie/path/multipart implementations','unselected locales','performance','Rust instrumentation']}
    for variant,text in [('native',original),('rust',select_overlay(selected.read_text()))]:
        fragment=body(text,marker)
        pieces.append((variant, 'namespace '+variant+' {static int parse_header(const std::shared_ptr<Http2Session>&client,HttpContext*ctx,int flags,const char*in,size_t inlen){'+fragment+'}}\n'))
    prelude=r'''
#include "swoole_http2.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <string>
#include <vector>
#include <memory>
#include <iostream>
#include <cstdarg>
#include <clocale>
#include <stdexcept>
#include <cstdlib>
using namespace swoole;
namespace Http2=swoole::http2;
#undef SW_HAVE_COMPRESSION
#if TEST_ZLIB
#define SW_HAVE_COMPRESSION
#endif
#define ZEND_STRL(s) s,sizeof(s)-1
#define NGHTTP2_HD_INFLATE_EMIT 2
#define NGHTTP2_HD_INFLATE_FINAL 1
#define PARSE_STRING 3
#define SW_HTTP_REQUEST_ENTITY_TOO_LARGE 413
using zend_long=long;
struct zval {int id=0;};
struct zend_string {std::string bytes;size_t length;};
#define ZSTR_VAL(s) ((s)->bytes.data())
#define ZSTR_LEN(s) ((s)->length)
struct nghttp2_hd_inflater {};
struct nghttp2_nv {uint8_t *name,*value;size_t namelen,valuelen;};
inline std::vector<std::string> effects;
inline std::vector<zend_string*> strings;
inline std::vector<char*> copies;
inline int new_error,decode_error,emit_header,throw_effect,boundary_ok;
inline unsigned decoded,script_kind;
inline std::string field,value;
inline int swoole_http_request_ce;
inline void event(std::string text){effects.push_back(text);if(throw_effect>0&&effects.size()==size_t(throw_effect))throw std::runtime_error("native effect");}
inline void emit(const char *fmt,...){char b[512];va_list v;va_start(v,fmt);vsnprintf(b,sizeof(b),fmt,v);va_end(v);event(b);}
#undef swoole_trace_log
#define swoole_trace_log(flags,...) emit(__VA_ARGS__)
#undef swoole_warning
#define swoole_warning(...) emit(__VA_ARGS__)
inline zend_string *zend_string_init(const char*p,size_t n,bool){event("string:"+std::string(p,n));auto s=new zend_string{std::string(p,n),n};strings.push_back(s);return s;}
inline zend_string *zend_string_dup(zend_string*s,bool){event("duplicate:"+s->bytes);return zend_string_init(s->bytes.data(),s->length,false);}
inline char *estrndup(const char*p,size_t n){event("copy:"+std::string(p,n));auto copy=new char[n+1];std::memcpy(copy,p,n);copy[n]=0;copies.push_back(copy);return copy;}
inline void add_assoc_stringl_ex(zval*z,const char*k,size_t kn,const char*v,size_t vn){event("assoc:"+std::to_string(z->id)+":"+std::string(k,kn)+":"+std::string(v,vn));}
inline void add_assoc_str_ex(zval*z,const char*k,size_t kn,zend_string*s){event("assoc-string:"+std::to_string(z->id)+":"+std::string(k,kn)+":"+s->bytes.substr(0,s->length));}
inline size_t php_url_decode(char*p,size_t n){event("url-decode:"+std::string(p,n));return n;}
inline zval properties[2]={{3},{4}};
inline zval *swoole_http_init_and_read_property(int,zval*,zval**slot,const char*p,size_t n){event("property:"+std::string(p,n));*slot=&properties[p[0]=='g'?0:1];return *slot;}
struct Sapi {void treat_data(int,char*p,zval*z){event("query:"+std::to_string(z->id)+":"+p);}};
inline Sapi sapi_module;
inline void swoole_http_parse_cookie(zval*z,const char*p,size_t n){event("cookie:"+std::to_string(z->id)+":"+std::string(p,n));}
struct Scratch {char str[256];};
inline Scratch scratch;
#define sw_tg_buffer fixture_buffer
inline Scratch *fixture_buffer(){event("buffer");return &scratch;}
struct HttpContext {struct Request {zval *zheader,*zserver,*zobject,*zget=nullptr,*zcookie=nullptr;char*path=nullptr;size_t path_len=0;bool post_form_urlencoded=false;} request;
 struct Parser {void *data=nullptr;} parser;bool enable_compression=false;unsigned boundary_calls=0,init_calls=0,compression_calls=0,status_calls=0;
 bool get_multipart_boundary(char*,size_t,size_t offset,char**p,int*n){event("boundary:"+std::to_string(offset));++boundary_calls;*p=(char*)"native-boundary";*n=15;return boundary_ok;}
 void init_multipart_parser(char*p,int n){event("multipart:"+std::string(p,n));++init_calls;}
 void set_compression_method(char*p,size_t n){event("compression:"+std::string(p,n));++compression_calls;}
};
inline bool http2_server_send_status_code(HttpContext*ctx,int code){event("status:"+std::to_string(code));++ctx->status_calls;return true;}
struct Http2Session {nghttp2_hd_inflater*inflater=nullptr;unsigned max_body_size=16;};
inline nghttp2_hd_inflater inflater_store;
inline void *php_nghttp2_mem(){event("memory");return nullptr;}
inline int nghttp2_hd_inflate_new2(nghttp2_hd_inflater**out,void*){event("new-inflater");*out=&inflater_store;return new_error;}
inline const char *nghttp2_strerror(ssize_t){return "native-error";}
inline ssize_t nghttp2_hd_inflate_hd(nghttp2_hd_inflater*,nghttp2_nv*nv,int*flags,uint8_t*,size_t available,int){
 event("inflate:"+std::to_string(available));if(decode_error&&decoded++==0)return -1;
 if(decoded++>=(script_kind?2u:1u)){*flags=NGHTTP2_HD_INFLATE_FINAL;return 0;}
 *nv={(uint8_t*)field.data(),(uint8_t*)value.data(),field.size(),value.size()};*flags=(script_kind&&decoded==1?0:NGHTTP2_HD_INFLATE_FINAL)|(emit_header?NGHTTP2_HD_INFLATE_EMIT:0);return available?1:0;
}
inline void nghttp2_hd_inflate_end_headers(nghttp2_hd_inflater*){event("end-headers");}
'''
    driver=r'''
struct Result {std::vector<std::string> events;int returned=-2;bool inflater=false,form=false,parser=false;unsigned boundary=0,init=0,compression=0,status=0;size_t path_len=0;std::string path,exception;};
template<bool rust>Result execute(bool existing,unsigned maximum,bool compression,int flags){
 effects.clear();decoded=0;auto client=std::make_shared<Http2Session>();client->inflater=existing?&inflater_store:nullptr;client->max_body_size=maximum;HttpContext ctx;zval header{1},server{2},object{5};ctx.request.zheader=&header;ctx.request.zserver=&server;ctx.request.zobject=&object;ctx.enable_compression=compression;char input[32]{};Result r;
 try{r.returned=rust?::rust::parse_header(client,&ctx,flags,input,script_kind?(flags&SW_HTTP2_FLAG_PRIORITY?6:1):sizeof(input)) : ::native::parse_header(client,&ctx,flags,input,script_kind?(flags&SW_HTTP2_FLAG_PRIORITY?6:1):sizeof(input));}catch(const std::runtime_error&e){r.exception=e.what();}
 r.events=effects;r.inflater=client->inflater!=nullptr;r.form=ctx.request.post_form_urlencoded;r.parser=ctx.parser.data==&ctx;r.boundary=ctx.boundary_calls;r.init=ctx.init_calls;r.compression=ctx.compression_calls;r.status=ctx.status_calls;r.path_len=ctx.request.path_len;if(ctx.request.path)r.path=std::string(ctx.request.path,ctx.request.path_len);
 for(auto s:strings)delete s;strings.clear();for(auto p:copies)delete[]p;copies.clear();return r;
}
static bool same(const Result&a,const Result&b){return a.events==b.events&&a.returned==b.returned&&a.inflater==b.inflater&&a.form==b.form&&a.parser==b.parser&&a.boundary==b.boundary&&a.init==b.init&&a.compression==b.compression&&a.status==b.status&&a.path_len==b.path_len&&a.path==b.path&&a.exception==b.exception;}
int main(){size_t count=0;std::vector<std::string> names={"",":method",":METHOD",":path",":PATH",":authority",":unknown","cookie","Cookie","content-type","CONTENT-TYPE","content-length","Content-Length","accept-encoding","x-other"};
std::vector<std::string> values={"","GET","/","/a?x=y","/a?","?x=y","/a?x?y","a=b;c=d","application/x-www-form-urlencoded","multipart/form-data; boundary=a","gzip","-1","0","16","17","invalid","17x",std::string("12\0tail",7)};
std::vector<std::string> locales,unavailable_locales;
for(const char*locale:{"C","C.UTF-8","en_US.UTF-8","tr_TR.ISO8859-9"}){if(!setlocale(LC_CTYPE,locale)){unavailable_locales.push_back(locale);continue;}locales.push_back(locale);
 for(auto&name:names)for(auto&v:values)for(unsigned scenario=0;scenario<128;++scenario){field=name;value=v;new_error=(scenario&1)?-1:0;decode_error=(scenario&2)?1:0;emit_header=(scenario&4)?0:1;boundary_ok=(scenario&8)?0:1;script_kind=(scenario&16)?1:0;throw_effect=0;
 auto a=execute<false>(scenario&32,scenario&8?0:16,scenario&64,scenario&16?SW_HTTP2_FLAG_PRIORITY:0);auto b=execute<true>(scenario&32,scenario&8?0:16,scenario&64,scenario&16?SW_HTTP2_FLAG_PRIORITY:0);if(!same(a,b)){std::cerr<<"server headers mismatch locale="<<locale<<" name="<<name<<" value="<<v<<" scenario="<<scenario<<"\n";return 1;}++count;
 }
}
for(auto&name:names)for(int stage=1;stage<24;++stage){field=name;value=name==":path"?"/a?x=y":name=="content-type"?"multipart/form-data; boundary=a":"17";new_error=decode_error=0;emit_header=boundary_ok=1;script_kind=1;throw_effect=stage;auto a=execute<false>(false,16,true,0);auto b=execute<true>(false,16,true,0);if(!same(a,b)){std::cerr<<"server exception prefix mismatch\n";return 2;}++count;}
std::cout<<"{\"passed\":true,\"server_header_cases\":"<<count<<",\"compression_enabled\":"<<TEST_ZLIB<<",\"locales\":[";for(size_t i=0;i<locales.size();++i){if(i)std::cout<<",";std::cout<<"\""<<locales[i]<<"\"";}std::cout<<"],\"unavailable_locales\":[";for(size_t i=0;i<unavailable_locales.size();++i){if(i)std::cout<<",";std::cout<<"\""<<unavailable_locales[i]<<"\"";}std::cout<<"]}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    commands, report['variants'] = sides.variants(work, 'headers', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include, define='TEST_ZLIB')
    report['commands'] += commands
    report['passed']=len(report['variants'])==2 and all(r['passed'] for r in report['variants']);report['native_sanitizers']=['address','undefined except alignment'];report['rust_sanitized']=False
    args.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({'passed':report['passed'],'report':str(args.output)}));return 0 if report['passed'] else 1

if __name__=='__main__':raise SystemExit(main())
