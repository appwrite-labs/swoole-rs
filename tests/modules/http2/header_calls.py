#!/usr/bin/env python3
"""Rust header role/media lookup against pinned locale-sensitive predicates.

The client header parser that used them moved to crates/phphttp2 (#827); its PHPTs
cover it. The lookups it calls stay in crates/http2 and are checked here.
"""
import argparse
import json
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent))
import gate
import sides


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
    report={'passed':False,'scope':__doc__,'upstream_commit':gate.PIN,'commands':[],
            'gaps':['unselected locales','performance','Rust instrumentation']}
    # The pinned predicates are the native side; the driver calls the Rust lookups.
    pieces=[('native', r'''
unsigned expected(unsigned role,const std::string&s){const char*p=s.data();size_t n=s.size();
 if(role==0){if(s.size()&&s[0]==':'){p++;n--;if(SW_STRCASEEQ(p,n,"method"))return 1;if(SW_STRCASEEQ(p,n,"path"))return 2;if(SW_STRCASEEQ(p,n,"authority"))return 3;}
 else{if(SW_STRCASEEQ(p,n,"content-type"))return 4;if(SW_STRCASEEQ(p,n,"cookie"))return 5;if(SW_STRCASEEQ(p,n,"content-length"))return 6;if(SW_STRCASEEQ(p,n,"accept-encoding"))return 7;}}
 if(role==1){if(s.size()&&s[0]==':'){if(SW_STRCASEEQ(p+1,n-1,"status"))return 8;}else{if(SW_STRCASEEQ(p,n,"content-encoding"))return 9;if(SW_STRCASEEQ(p,n,"set-cookie"))return 10;}}
 if(role==2){if(SW_STRCASEEQ(p,n,"server"))return 11;if(SW_STRCASEEQ(p,n,"content-length"))return 6;if(SW_STRCASEEQ(p,n,"date"))return 12;if(SW_STRCASEEQ(p,n,"content-type"))return 4;}
 if(role==3&&SW_STRCASEEQ(p,n,"host"))return 13;return 0;}
unsigned expected_media(const std::string&v){return SW_STR_ISTARTS_WITH(v.data(),v.size(),"application/x-www-form-urlencoded")?1:SW_STR_ISTARTS_WITH(v.data(),v.size(),"multipart/form-data")?2:SW_STR_ISTARTS_WITH(v.data(),v.size(),"gzip")?3:0;}
''')]
    prelude=r'''
#include "swoole_http2.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <string>
#include <vector>
#include <iostream>
#include <clocale>
#include <cctype>
#include <strings.h>
using namespace swoole;
namespace Http2=swoole::http2;
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
int main(){size_t count=0;std::vector<std::string> names={"",":status",":STATUS",":method",":path",":authority","content-type","CONTENT-TYPE","content-length","accept-encoding","content-encoding","set-cookie","Set-Cookie","server","date","HOST","cookie",std::string(200,'a')};
for(unsigned c=0;c<256;++c){names.push_back(std::string(1,char(c))+"status");names.push_back("content-"+std::string(1,char(c))+"ype");names.push_back("set-cookie"+std::string(1,char(c)));}
std::vector<std::string> values={"","200","00301","application/x-www-form-urlencoded","APPLICATION/X-WWW-FORM-URLENCODED; charset=x","multipart/form-data; boundary=a","MULTIPART/FORM-DATA","gzip","GZip;stuff","gzipgarbage","a=b;c=d",std::string("gzip\0tail",9),std::string(40,'a')};
std::vector<std::string> locales,unavailable_locales;for(const char *locale:{"C","C.UTF-8","en_US.UTF-8","tr_TR.ISO8859-9"}){if(!setlocale(LC_CTYPE,locale)){unavailable_locales.push_back(locale);continue;}locales.push_back(locale);
 for(auto&name:names)for(unsigned role=0;role<4;++role){if(expected(role,name)!=Http2::rust_header_kind((SwRsHttp2HeaderRole)role,name.data(),name.size())){std::cerr<<"header kind mismatch\n";return 1;}++count;}
 for(auto&v:values){if(expected_media(v)!=Http2::rust_media_kind(v.data(),v.size()))return 2;++count;}
}
std::cout<<"{\"passed\":true,\"header_cases\":"<<count<<",\"locales\":[";for(size_t i=0;i<locales.size();++i){if(i)std::cout<<",";std::cout<<"\""<<locales[i]<<"\"";}std::cout<<"],\"unavailable_locales\":[";for(size_t i=0;i<unavailable_locales.size();++i){if(i)std::cout<<",";std::cout<<"\""<<unavailable_locales[i]<<"\"";}std::cout<<"]}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    report['commands'], result = sides.run(work, 'headers', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include)
    report.update(result or {})
    report['native_sanitizers']=['address','undefined except alignment'];report['rust_sanitized']=False
    args.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({'passed':report['passed'],'report':str(args.output)}));return 0 if report['passed'] else 1

if __name__=='__main__':raise SystemExit(main())
