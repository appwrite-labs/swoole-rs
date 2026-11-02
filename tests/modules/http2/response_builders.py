#!/usr/bin/env python3
"""Actual server header/trailer builders and native HeaderSet allocation/publication bodies."""
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
    name = 'ext-src/swoole_http2_server.cc'
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
                       'Original repeated content-type captures and exceptional allocation remnants are preserved, not repaired',
                       'Invalidated raw-pointer lifetimes and original undefined HeaderSet bounds are excluded',
                       'Rust sanitizer instrumentation and performance']}
    pieces = []
    for variant, text in [('native', original), ('rust', select_overlay(selected.read_text()))]:
        fragments = []
        for marker, signature in [
                ('static ssize_t http2_server_build_header(', 'ssize_t build_header(HttpContext*ctx,uchar*buffer,const BufferValue*body)'),
                ('static ssize_t http2_server_build_trailer(', 'ssize_t build_trailer(const HttpContext*ctx,uchar*buffer)')]:
            fragment = body(text, marker)
            fragments.append(signature + '{' + fragment + '}')
        pieces.append((variant, 'namespace ' + variant + ' {using String=BufferValue;' + ''.join(fragments) + '}'))
    warning_body = body(php_header, 'static sw_inline bool swoole_http_has_crlf(')
    flags = body(php_header, 'enum swHttpHeaderFlag')
    prelude = r'''
#include "swoole_http2.h"
#include "swoole_util.h"
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
#include <set>
#include <cassert>
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
#undef SW_HAVE_COMPRESSION
#if TEST_COMPRESSION
#define SW_HAVE_COMPRESSION
#endif
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
struct zend_string {std::string bytes;size_t length=0;};
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
#define ZSTR_LEN(z) ((z)->length)
#define ZEND_HASH_FOREACH_STR_KEY_VAL(table,out_key,out_value) for(auto&entry:(table)->entries) if(((out_key=entry.key),(out_value=entry.value),true))
#define UNEXPECTED(x) (x)
#define ZEND_HASH_FOREACH_END()
#define SW_ZSTR_KNOWN(x) (x)
#define SW_ZEND_STR_HEADER 0
#define SW_ZEND_STR_COOKIE 1
#define SW_ZEND_STR_TRAILER 2
#define SW_HTTP_OK 200
#define SW_HASHTABLE_FOREACH_START2(table,out_key,out_len,out_type,out_value) for(auto&entry:(table)->entries) if(((out_key=entry.key?entry.key->bytes.data():nullptr),(out_len=entry.key?entry.key->length:0),(out_type=0),(out_value=entry.value),true))
#define SW_HASHTABLE_FOREACH_START(table,out_value) for(auto&entry:(table)->entries) if(((out_value=entry.value),true))
#define SW_HASHTABLE_FOREACH_END()
inline int swoole_http_response_ce;
inline zval properties[3];
inline zval*sw_zend_read_property_ex(int,zval*,int field,int){event("property:"+std::to_string(field));return &properties[field];}
inline size_t php_swoole_array_length_safe(zval*z){event("array-length");return ZVAL_IS_ARRAY(z)?z->entries.size():0;}
namespace zend {struct String {std::string value;explicit String(zval*z):value(z->bytes){event("convert:"+value);}~String(){event("string-drop",false);}void rtrim(){event("trim");value.resize(swoole::rtrim(value.data(),value.size()));}char*val(){return value.data();}size_t len(){return value.size();}};}
inline std::set<zend_string*> captures;
inline zend_string*zval_get_string(zval*z){event("capture:"+z->bytes);auto s=new zend_string{z->bytes,z->bytes.size()};captures.insert(s);return s;}
inline void zend_string_release(zend_string*z){event("release:"+z->bytes,false);if(!captures.erase(z))std::abort();delete z;}
inline void php_swoole_error(int,const char*what){event("warning:"+std::string(what));}
struct BufferValue {char storage[64];char*str=storage;size_t length=0;};
struct HttpContext {struct Response {zval*zobject;int status=0;}response;unsigned send_header_=0;int fd=1;bool accept_compression=false;std::set<std::string>*compression_types=nullptr;BufferValue*zlib_buffer;bool compression_result=false;
 bool compress(const char*p,size_t n){event("compress:"+std::string(p,n));return compression_result;}
 const char*get_content_encoding(){event("encoding");return "gzip";}
};
inline zend_string date_value{"date",4};
inline zend_string*php_swoole_http_get_date(){event("date");return &date_value;}
template<class T>inline int fixture_itoa(char*p,T value){event("integer");auto text=std::to_string(value);if(text.size()>=16)std::abort();std::memcpy(p,text.data(),text.size());return text.size();}
#define swoole_itoa fixture_itoa
struct Http2Session {void*deflater=nullptr;swoole::http2::Settings remote_settings{};};
inline std::map<int,std::shared_ptr<Http2Session>> http2_sessions;
inline unsigned new_error,deflate_error;
inline void*php_nghttp2_mem(){event("memory");return nullptr;}
inline int nghttp2_hd_deflate_new2(void**out,unsigned size,void*){event("new-deflater:"+std::to_string(size));*out=(void*)1;return new_error?-1:0;}
inline const char*nghttp2_strerror(int){return "native-error";}
#undef swoole_warning
#define swoole_warning(...) event("native-warning")
inline size_t nghttp2_hd_deflate_bound(void*,nghttp2_nv*values,size_t n){event("bound:"+std::to_string(n));for(size_t i=0;i<n;++i)event("nv:"+std::to_string(i)+":"+std::string((char*)values[i].name,values[i].namelen)+":"+std::string((char*)values[i].value,values[i].valuelen));return 128;}
inline ssize_t nghttp2_hd_deflate_hd(void*,unsigned char*buffer,size_t bound,nghttp2_nv*,size_t n){event("deflate:"+std::to_string(bound)+":"+std::to_string(n));if(deflate_error)return -1;std::memset(buffer,'C',7);return 7;}
'''
    prelude += 'enum swHttpHeaderFlag {' + flags + '};\ninline bool swoole_http_has_crlf(const char*value,size_t length){' + warning_body + '}\n'
    driver = r'''
struct Result {std::vector<std::string>events;ssize_t returned=-2;unsigned remaining=0,captures_left=0,sent=0;int status=0;bool accept=false,types=false,deflater=false;std::string exception,output;};
template<bool rust>Result execute(bool trailer,int status,unsigned pattern,bool body,unsigned compression,unsigned failure,unsigned change,unsigned throw_index){
 effects.clear();serial=0;throw_at=throw_index;mutation=change;new_error=failure&1;deflate_error=failure&2;HttpContext ctx;zval object;ctx.response.zobject=&object;ctx.response.status=status;ctx.accept_compression=compression&1;std::set<std::string>types={"text/html","application/json","application/json "};ctx.compression_types=compression&2?&types:nullptr;ctx.compression_result=compression&4;BufferValue data,zlib;std::memset(data.str,'B',sizeof(data.storage));data.length=body?5:0;zlib.length=3;ctx.zlib_buffer=&zlib;
 properties[0]=zval{2,"",{}};properties[1]=zval{2,"",{}};properties[2]=zval{2,"",{}};
 zend_string server{"server",6},type{"content-type",12},length{"content-length",14},date{"date",4},other{"x-extra",7};zval string{1,"application/json ",{}},empty{1,"",{}},null_value{0,"",{}},number{3,"17",{}},crlf{1,"bad\r\ninside",{}},nested{2,"",{{nullptr,&string},{nullptr,&null_value},{nullptr,&empty}}};
 std::vector<Entry>entries;
 if(pattern==1)entries={{&server,&string},{&date,&empty},{&length,&number}};
 if(pattern==2)entries={{&type,&string},{&other,&crlf}};
 if(pattern==3)entries={{nullptr,&string},{&other,&null_value},{&type,&nested}};
 if(pattern==4)entries={{&type,&string},{&type,&empty},{&other,&number}};
 properties[0].entries=entries;properties[2].entries=entries;properties[1].entries={{nullptr,&string},{nullptr,&number},{nullptr,&empty}};
 auto session=std::make_shared<Http2Session>();session->deflater=failure&4?(void*)1:nullptr;session->remote_settings.header_table_size=4096;http2_sessions[1]=session;
 mutate=[&](const std::string&what){if(what.rfind("convert:",0)==0){if(change&1)ctx.accept_compression=!ctx.accept_compression;if(change&2)ctx.compression_types=ctx.compression_types?nullptr:&types;}if((change&4)&&what=="compress:"+std::string(data.str,data.length))zlib.length=7;};
 unsigned char output[256]{};Result r;try{r.returned=trailer?(rust?::rust::build_trailer(&ctx,output) : ::native::build_trailer(&ctx,output)):(rust?::rust::build_header(&ctx,output,body?&data:nullptr) : ::native::build_header(&ctx,output,body?&data:nullptr));}catch(const std::runtime_error&e){r.exception=e.what();}
 r.events=effects;r.remaining=allocations.size();r.captures_left=captures.size();r.sent=ctx.send_header_;r.status=ctx.response.status;r.accept=ctx.accept_compression;r.types=ctx.compression_types!=nullptr;r.deflater=session->deflater!=nullptr;if(r.returned>0)r.output.assign((char*)output,r.returned);mutate={};for(auto [pointer,id]:allocations)std::free(pointer);allocations.clear();for(auto z:captures)delete z;captures.clear();http2_sessions.clear();return r;
}
static bool same(const Result&a,const Result&b){return a.events==b.events&&a.returned==b.returned&&a.remaining==b.remaining&&a.captures_left==b.captures_left&&a.sent==b.sent&&a.status==b.status&&a.accept==b.accept&&a.types==b.types&&a.deflater==b.deflater&&a.exception==b.exception&&a.output==b.output;}
int main(){size_t count=0;
for(bool trailer:{false,true})for(int status:{0,200,404,-1,INT32_MIN,INT32_MAX})for(unsigned pattern=0;pattern<5;++pattern)for(bool body:{false,true})for(unsigned compression=0;compression<8;++compression)for(unsigned failure=0;failure<8;++failure)for(unsigned change=0;change<8;++change){
 auto a=execute<false>(trailer,status,pattern,body,compression,failure,change,0);auto b=execute<true>(trailer,status,pattern,body,compression,failure,change,0);if(!same(a,b)){std::cerr<<"response builder mismatch trailer="<<trailer<<" status="<<status<<" pattern="<<pattern<<" body="<<body<<" compression="<<compression<<" failure="<<failure<<" mutation="<<change<<"\n";return 1;}++count;
}
for(bool trailer:{false,true})for(unsigned pattern=0;pattern<5;++pattern)for(unsigned throw_index=1;throw_index<70;++throw_index){auto a=execute<false>(trailer,0,pattern,true,7,0,3,throw_index);auto b=execute<true>(trailer,0,pattern,true,7,0,3,throw_index);if(!same(a,b)){std::cerr<<"response builder prefix mismatch\n";return 2;}++count;}
std::cout<<"{\"passed\":true,\"response_builder_cases\":"<<count<<",\"compression_enabled\":"<<TEST_COMPRESSION<<"}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    commands, report['variants'] = sides.variants(work, 'response', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include, define='TEST_COMPRESSION')
    report['commands'] += commands
    report['passed'] = len(report['variants']) == 2 and all(r['passed'] for r in report['variants'])
    report['native_sanitizers'] = ['address', 'undefined except alignment']
    report['rust_sanitized'] = False
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'report': str(args.output)}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
