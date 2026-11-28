#!/usr/bin/env python3
"""Normal exact Port getter policy; real Zend lowercase table, native models, no callback execution/lifetime profiles."""
import argparse
from pathlib import Path
import json
import subprocess
import sys
import overlay
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.build_ext import native_flags
from tools.harness import run_isolated,write_report


def zend_map(folder, php_config):
    php=subprocess.check_output([php_config,'--php-binary'],text=True).strip()
    # Read exactly the exported 256-byte initialized immutable Zend table.
    script='$f=FFI::cdef("extern const unsigned char zend_tolower_map[256];");$s="";for($i=0;$i<256;$i++)$s.=chr($f->zend_tolower_map[$i]);echo json_encode(["php"=>PHP_VERSION,"zts"=>PHP_ZTS,"hex"=>bin2hex($s)]);'
    command=[php,'-r',script];p=subprocess.run(command,capture_output=True,text=True,check=True)
    assert not p.stderr,p.stderr
    facts=json.loads(p.stdout);data=bytes.fromhex(facts['hex'])
    assert data==bytes(c+32 if 65<=c<=90 else c for c in range(256))
    # The pinned event catalog is the oracle on both sides: the hybrid tree moved it to Rust.
    header=overlay.original('ext-src/php_swoole_server.h');start=header.index('enum php_swoole_server_port_callback_type {');end=header.index('};',start)+2
    native=overlay.original('ext-src/swoole_server_port.cc');a=native.index('struct ServerPortEvent {');b=native.index('// clang-format on',a)
    (folder/'events.inc').write_text(header[start:end]+'\n'+native[a:b])
    path=folder/'zend-map.inc';path.write_text('static const unsigned char zend_lowercase[256] = {'+','.join(str(c)for c in data)+'};\n')
    return {'scope':'Actual exported Zend ASCII lowercase table; read-only initialized256bytes via PHP FFI, no Port/runtime hook','command':command,'exit':p.returncode,'stdout':p.stdout,'stderr':p.stderr,'facts':facts}


# The hybrid's getCallback is crates/phpserver/src/port.rs `get_callback`: zval_get_string,
# the Rust lowering swr_v1_ports_callback_property, a silent property read and ZVAL_COPY.
# This is that body in the witness's terms, so the Rust lowering meets the original's.
RUST_GET_CALLBACK='''static PHP_METHOD(swoole_server_port, getCallback) {
    zval *name;

    ZEND_PARSE_PARAMETERS_START(1, 1)
    Z_PARAM_ZVAL(name)
    ZEND_PARSE_PARAMETERS_END_EX(RETURN_FALSE);

    zend::String event_name(name);
    auto property_name = swr_v1_ports_callback_property((const uint8_t *) event_name.val(), event_name.len());
    if (property_name.data) {
        zval rv, *property = zend_read_property(swoole_server_port_ce, SW_Z8_OBJ_P(ZEND_THIS),
                                                (const char *) property_name.data, property_name.len, true, &rv);
        if (!ZVAL_IS_NULL(property)) {
            RETURN_ZVAL(property, 1, 0);
        }
    }
    RETURN_NULL();
}
'''


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True,help='Rust archive of the hybrid build under test');p.add_argument('--output',type=Path,required=True);p.add_argument('--php-config',default='php-config');p.add_argument('--sanitize',action='store_true');a=p.parse_args();overlay.SOURCE=a.source
    folder=a.output.resolve().with_suffix('');folder.mkdir(parents=True,exist_ok=True)
    # The original getter from the pin; the selected one is the Rust getter above, linked from the archive.
    native=overlay.original('ext-src/swoole_server_port.cc');start=native.index('static PHP_METHOD(swoole_server_port, getCallback) {')
    report={'scope':__doc__,'pin':overlay.PIN,'passed':False}
    for side,body in [('original',native[start:native.index('\n}\n',start)+3]),('selected',RUST_GET_CALLBACK)]:
        (folder/side).mkdir(exist_ok=True);(folder/side/'body.inc').write_text(body)
        report['zend_map']=zend_map(folder/side,a.php_config)
    # Two translation units: the original side sees only pinned text, the selected side and the driver the hybrid build's flags.
    common=['c++','-std=c++17','-O1','-UNDEBUG']
    if a.sanitize:common+=['-fsanitize=address,undefined','-fno-sanitize-recover=all','-g']
    witness=str(Path(__file__).with_suffix('.cc'))
    original=[*common,'-c','-DPORTS_SIDE=original','-I'+str(folder/'original'),witness,'-o',str(folder/'original.o')]
    command=[*common,'-DPORTS_SIDE=selected','-DPORTS_DRIVER','-I'+str(folder/'selected'),*native_flags(a.archive),witness,str(folder/'original.o'),str(a.archive.resolve()),'-lpthread',*(['-Wl,--gc-sections','-ldl','-lm'] if sys.platform=='linux' else []),'-o',str(folder/'gate')]
    r=run_isolated(original,timeout=90)
    if not r.returncode:r=run_isolated(command,timeout=90)
    report['compile']={'commands':[original,command],'exit':r.returncode,'stdout':r.stdout,'stderr':r.stderr}
    if not r.returncode:
        r=run_isolated([str(folder/'gate')],timeout=30);report['run']={'exit':r.returncode,'stdout':r.stdout,'stderr':r.stderr};report['passed']=r.returncode==0
    write_report(a.output,report);print(json.dumps({'passed':report['passed'],'stdout':r.stdout,'stderr':r.stderr}));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
