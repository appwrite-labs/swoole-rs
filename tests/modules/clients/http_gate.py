#!/usr/bin/env python3
"""Pinned/patched Set-Cookie parsing: Zend-effect order comparison.

Native effects are recording doubles; no actual PHP/network parity is implied.
"""
import argparse
import json
from pathlib import Path
import gate

ROOT = Path(__file__).resolve().parents[3]

def main():
    p=argparse.ArgumentParser()
    p.add_argument('--source',type=Path,required=True)
    p.add_argument('--archive',type=Path)
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--sanitize',action='store_true')
    p.add_argument('--rust-sanitized',action='store_true')
    a=p.parse_args();out=a.output.parent/(a.output.stem+'-artifacts');out.mkdir(parents=True,exist_ok=True)
    gate.LOG=out/'commands';gate.LOG.mkdir(exist_ok=True)
    if a.rust_sanitized and not (a.archive and a.sanitize):p.error('--rust-sanitized needs --archive and --sanitize')
    archive=a.archive or out/'libclients.a'
    if not a.archive:
        gate.run(['rustc','--edition','2024','--crate-type','staticlib','-O','-C','panic=abort',ROOT/'crates/clients/src/lib.rs','-o',archive])
    scratch=out/'patched';scratch.mkdir(exist_ok=True);gate.run(['git','init','-q',scratch])
    paths=gate.overlay_manifest()['upstream_paths']
    original=''
    for path in paths:
        text=gate.run(['git','-C',a.source,'show',f'{gate.PIN}:{path}']);dest=scratch/path;dest.parent.mkdir(parents=True,exist_ok=True);dest.write_text(text)
        if path=='ext-src/swoole_http_client_coro.cc':original=text
    gate.apply_overlay(scratch, out)
    selected=(scratch/'ext-src/swoole_http_client_coro.cc').read_text()
    signatures=['void php_swoole_http_parse_set_cookies(']
    bodies={name:'\n'.join(gate.body(text,sig) for sig in signatures) for name,text in [('native',original),('rust',selected)]}
    for signature in signatures:
        native_body=gate.body(original,signature);selected_body=gate.body(selected,signature)
        if native_body==selected_body or 'swr_v1_clients_' not in selected_body:
            raise ValueError(f'Selected body was not converted: {signature}')
    prelude=r'''
#include <cstring>
#include <cstdint>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>
#include "swoole_rs_clients.h"
static std::vector<std::string> zend_effects;
struct zval {std::string bytes;size_t length=0;};
#define ZVAL_STRINGL(z,p,n) ((z)->bytes.assign(p,n),(z)->length=n,zend_effects.push_back("value:"+std::string(p,n)))
#define ZVAL_EMPTY_STRING(z) ((z)->bytes.clear(),(z)->length=0,zend_effects.push_back("empty"))
#define Z_STRVAL(z) ((z).bytes.data())
#define Z_STRLEN(z) ((z).length)
// The native PHP decoder itself remains external; preserve its exact call bytes.
static size_t php_url_decode(char *p,size_t n) {zend_effects.push_back("decode:"+std::string(p,n));return n;}
static void add_next_index_zval(zval*,zval*v) {zend_effects.push_back("append:"+v->bytes.substr(0,v->length));}
static void add_assoc_zval_ex(zval*,const char*k,size_t n,zval*v) {zend_effects.push_back("assoc:"+std::string(k,n)+":"+v->bytes.substr(0,v->length));}
static void add_next_index_stringl(zval*,char*p,size_t n) {zend_effects.push_back("raw:"+std::string(p,n));}
'''
    driver=r'''
int main() {
    size_t count=0;
    std::vector<std::string> cookies={"","a=b","=b","abc","a=","a=b;c=d","a;b=c","a=b=c","a=%20+; path=/",std::string("a\0x=b\0c;d",sizeof("a\0x=b\0c;d")-1)};
    for(unsigned i=0;i<256;i++)for(const auto&prefix:std::vector<std::string>{"","a=","a;", "="})cookies.push_back(prefix+std::string(1,char(i))+"=x;y");
    for(const auto&cookie:cookies) {
        zval a,b;zend_effects.clear();native::php_swoole_http_parse_set_cookies(cookie.data(),cookie.size(),&a,&b);auto expected=zend_effects;
        zend_effects.clear();rust::php_swoole_http_parse_set_cookies(cookie.data(),cookie.size(),&a,&b);
        if(expected!=zend_effects)return 1;++count;
    }
    std::cout<<count<<"\n";
}
'''
    cpp=out/'actual_http_callers.cpp';cpp.write_text(prelude+'\nnamespace native {\n'+bodies['native']+'\n}\nnamespace rust {\n'+bodies['rust']+'\n}\n'+driver)
    exe=out/'actual_http_callers';flags=['-fsanitize=address,undefined','-fno-omit-frame-pointer'] if a.sanitize else []
    gate.run(['c++','-std=c++17','-O1',*flags,'-I',ROOT/'native/clients',cpp,archive,'-lpthread','-ldl','-lm','-o',exe]);cases=int(gate.run([exe]).strip())
    report=dict(passed=True,cases=cases,scope='actual extracted native/patched PHP Set-Cookie parsing bodies with Zend effect doubles; not PHP parity',pin=gate.PIN,archive=str(archive),native_sanitized=a.sanitize,rust_sanitized=a.rust_sanitized)
    a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))

if __name__=='__main__':main()
