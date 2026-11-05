#!/usr/bin/env python3
"""Compile the hybrid build's typed Buffer callers.

The callers are compiled from the archive's hybrid tree with its switches and headers
(hybrid_tree, #796), so the halfclose repair and the typed queue are whatever that
build selected. Syntax only, not runtime execution.
"""
import shutil
import argparse, json, subprocess, sys, tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.build_hybrid import hybrid_tree
from tools.build_upstream import COMMIT as PIN
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--report',type=Path,required=True);p.add_argument('--archive',type=Path,required=True,help='Rust archive of the hybrid build under test');a=p.parse_args()
 if min(shutil.disk_usage(ROOT).free,shutil.disk_usage(tempfile.gettempdir()).free)<4*1024**3:p.error("4GiB disk guard refused compilation")
 if a.report.exists():p.error('Refusing to replace receipt')
 tree,flags=hybrid_tree(a.archive)
 if '-DSWOOLE_RS_BUFFER_TYPED' not in flags:p.error('the hybrid build does not use the typed Buffer queue')
 report=dict(pin=PIN,scope=__doc__,hybrid_tree=str(tree),hybrid_flags=flags,rows=[])
 linux=sys.platform.startswith('linux')
 for name in ['src/core/buffer.cc','src/reactor/base.cc','src/server/reactor_thread.cc','src/server/master.cc']:
  command=['c++','-std=c++14','-fsyntax-only','-DHAVE_EPOLL=1' if linux else '-DHAVE_KQUEUE=1',*flags,'-I'+str(tree)]
  command+=subprocess.check_output(['pkg-config','--cflags','openssl'],text=True).split()
  command+=[str(tree/name)]
  result=subprocess.run(command,capture_output=True,text=True)
  report['rows'].append(dict(source=name,command=command,returncode=result.returncode,stdout=result.stdout,stderr=result.stderr,passed=result.returncode==0))
  print(name,result.returncode,result.stderr[:300],flush=True)
 report['passed']=bool(report['rows']) and all(r['passed'] for r in report['rows'])
 a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,indent=2)+'\n');return int(not report['passed'])
if __name__=='__main__':raise SystemExit(main())
