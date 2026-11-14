#!/usr/bin/env python3
"""swr_v1_clients_http_format against the pinned request formatter calls at bounded capacities.

The C++ callers moved to crates/phpclients (#819); clients-php-http_format and the
PHPTs cover them. No allocation, IO, Zend or whole request-runtime parity claim.
Native positive printf capacities are restricted to INT_MAX.
"""
import argparse,json,sys
from pathlib import Path
import gate
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.build_ext import native_flags
def pinned_defines(config):
 defines_list=[];lines=config.splitlines()
 for i,line in enumerate(lines):
  if any(line.startswith('#define '+name+' ') for name in ['SW_HTTP_CLIENT_BOUNDARY_TOTAL_SIZE','SW_HTTP_FORM_RAW_DATA_FMT','SW_HTTP_FORM_FILE_DATA_FMT']):
   define=line
   while define.endswith('\\'):
    i+=1;define+='\n'+lines[i]
   defines_list.append(define)
 return '\n'.join(defines_list)
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True,help='Rust archive of the hybrid build under test');p.add_argument('--output',type=Path,required=True);p.add_argument('--sanitize',action='store_true');p.add_argument('--rust-sanitized',action='store_true');a=p.parse_args()
 if a.rust_sanitized and not a.sanitize:p.error('--rust-sanitized requires --sanitize')
 out=a.output.parent/(a.output.stem+'-artifacts');out.mkdir(parents=True,exist_ok=True);gate.LOG=out/'commands';gate.LOG.mkdir(exist_ok=True)
 # Native side from the pin, Rust side against the hybrid build's fully patched tree.
 hybrid_flags=native_flags(a.archive)
 pinned=lambda path:gate.run(['git','-C',a.source,'show',f'{gate.PIN}:{path}'])
 printf=gate.body(pinned('src/core/base.cc'),'size_t sw_snprintf(char *buf, size_t size, const char *format, ...) {')
 template=Path(__file__).with_name('http_format_witness.cpp').read_text();native_text,rust_text=template.split('// ENTRY')
 # Each side in its own translation unit: the native one sees only pinned text.
 native,rust=out/'native_http_format.cpp',out/'rust_http_format.cpp'
 native.write_text(native_text.replace('// PINNED_DEFINES',pinned_defines(pinned('include/swoole_config.h'))).replace('// NATIVE_PRINTF',printf))
 rust.write_text(native_text.split('// PINNED_DEFINES')[0]+rust_text)
 exe=out/'http_format';flags=['-std=c++17','-O1',*(['-fsanitize=address,undefined','-fno-omit-frame-pointer'] if a.sanitize else [])]
 gate.run(['c++',*flags,'-c',native,'-o',out/'native_http_format.o'])
 gate.run(['c++',*flags,*hybrid_flags,rust,out/'native_http_format.o',a.archive,'-Wl,--gc-sections','-lpthread','-ldl','-lm','-o',exe]);result=json.loads(gate.run([exe]))
 report=dict(passed=True,results=result,scope=__doc__,pin=gate.PIN,native_sanitized=a.sanitize,rust_sanitized=a.rust_sanitized);a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
if __name__=='__main__':main()
