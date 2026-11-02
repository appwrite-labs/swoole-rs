#!/usr/bin/env python3
"""Actual client response publication suffix with native object/buffer/lifetime effects."""
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
    report = {'passed': False, 'scope': __doc__, 'upstream_commit': gate.PIN,
              'commands': [], 'gaps': [
                  'Real PHP property/reference-count/GC semantics, sockets, HPACK and coroutine scheduling',
                  'Native Zend longjmp bailout; C++ exception prefixes are separate fixture outcomes',
                  'Callbacks freeing a still-used native raw stream are outside defined fixture lifetimes',
                  'RST_STREAM and GOAWAY return earlier in the full parser; not counted as reachable publication cases',
                  'Rust sanitizer instrumentation and performance']}
    pieces = []
    for variant, text in [('native', original), ('rust', select_overlay(selected.read_text()))]:
        method = body(text, 'ReturnCode Client::parse_frame(')
        marker = '    bool end = (flags &' if variant == 'native' else '    SwRsHttp2Response response_plan;'
        suffix = method[method.index(marker):]
        pieces.append((variant, 'namespace ' + variant + ' {struct Client: FixtureClient {int publish(unsigned type, unsigned flags, unsigned _stream_id, bool pipeline_read){Stream *stream=&value;' + suffix + '}};}'))
    prelude = r'''
#include "swoole_http2.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <string>
#include <vector>
#include <iostream>
#include <stdexcept>
using namespace swoole;
inline std::vector<std::string> effects;
inline unsigned throw_at, mutation;
struct zval {unsigned identity=19;};
struct FixtureBuffer {char str[8]={'a',0,'b','c',0,0,0,0};size_t length=0;
 void clear();};
struct Stream {unsigned flags=0;zval zresponse;FixtureBuffer *buffer=nullptr;};
inline Stream *current;
inline void effect(std::string text){effects.push_back(text);if(mutation&1)current->flags^=15;if(mutation&2)current->zresponse.identity+=3;if(throw_at==effects.size())throw std::runtime_error("native effect");}
inline void FixtureBuffer::clear(){effect("clear:"+std::to_string(length));length=0;}
inline int swoole_http2_response_ce;
#define ZEND_STRL(x) x,sizeof(x)-1
#define SW_Z8_OBJ_P(x) (x)
#define SW_READY 2
#define SW_CONTINUE 0
#define HTTP_ESTATUS_SERVER_RESET -3
inline void zend_update_property_stringl(int,zval*z,const char*k,size_t n,const char*v,size_t len){effect("string:"+std::string(k,n)+":"+std::to_string(z->identity)+":"+std::string(v,len));}
inline void zend_update_property_bool(int,zval*z,const char*k,size_t n,int value){effect("bool:"+std::string(k,n)+":"+std::to_string(z->identity)+":"+std::to_string(value));}
inline void zend_update_property_long(int,zval*z,const char*k,size_t n,unsigned value){effect("long:"+std::string(k,n)+":"+std::to_string(z->identity)+":"+std::to_string(value));}
inline void object_init_ex(zval*z,int){effect("object:"+std::to_string(z->identity));z->identity=71;}
namespace zend {inline void object_set(zval*z,const char*k,size_t n,int value){effect("set:"+std::string(k,n)+":"+std::to_string(z->identity)+":"+std::to_string(value));}}
struct FixtureClient {Stream value;bool deleted=false;int returned_identity=-1;int stream_error=42;
 void retval(zval*z,bool copy,int destruct){effect("return:"+std::to_string(z->identity)+":"+std::to_string(copy)+":"+std::to_string(destruct));returned_identity=z->identity;}
 void delete_stream(unsigned id){effect("delete:"+std::to_string(id));deleted=true;}
};
#define RETVAL_ZVAL(z,copy,destruct) retval(z,copy,destruct)
'''
    driver = r'''
struct Result {std::vector<std::string> events;int returned=-1,identity=-1;unsigned flags=0,response=0;size_t remaining=0;bool deleted=false;std::string exception;};
template<class C>Result execute(unsigned type,unsigned frame_flags,unsigned stream_flags,bool pipeline,unsigned buffer_kind,unsigned fail,unsigned change){
 effects.clear();throw_at=fail;mutation=change;C client;FixtureBuffer buffer;buffer.length=buffer_kind>1?buffer_kind-1:0;client.value.flags=stream_flags;client.value.buffer=buffer_kind?&buffer:nullptr;current=&client.value;Result result;
 try{result.returned=client.publish(type,frame_flags,123,pipeline);}catch(const std::runtime_error&e){result.exception=e.what();}
 result.events=effects;result.identity=client.returned_identity;result.flags=client.value.flags;result.response=client.value.zresponse.identity;result.remaining=buffer.length;result.deleted=client.deleted;return result;
}
int main(){size_t count=0;
// Only kinds that can reach this suffix after the real parser's earlier control returns.
for(unsigned kind:{0u,1u,2u,9u,10u,255u})for(unsigned flags:{0u,1u,4u,5u,255u})for(unsigned stream=0;stream<32;++stream)for(bool pipeline:{false,true})for(unsigned buffer:{0u,1u,2u,5u})for(unsigned fail=0;fail<9;++fail)for(unsigned change=0;change<4;++change){
 auto a=execute<native::Client>(kind,flags,stream,pipeline,buffer,fail,change);auto b=execute<rust::Client>(kind,flags,stream,pipeline,buffer,fail,change);
 if(a.events!=b.events||a.returned!=b.returned||a.identity!=b.identity||a.flags!=b.flags||a.response!=b.response||a.remaining!=b.remaining||a.deleted!=b.deleted||a.exception!=b.exception){std::cerr<<"publication mismatch kind="<<kind<<" flags="<<flags<<" stream="<<stream<<" pipeline="<<pipeline<<" buffer="<<buffer<<" fail="<<fail<<" mutation="<<change<<"\n";return 1;}++count;
}
std::cout<<"{\"passed\":true,\"publication_cases\":"<<count<<"}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    report['commands'], result = sides.run(work, 'publication', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include)
    report.update(result or {})
    report['native_sanitizers'] = ['address', 'undefined except alignment']
    report['rust_sanitized'] = False
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'report': str(args.output)}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
