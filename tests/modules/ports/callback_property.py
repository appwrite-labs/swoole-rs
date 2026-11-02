#!/usr/bin/env python3
"""Normal exact Port getter policy; real Zend lowercase table, native models, no callback execution/lifetime profiles."""
from pathlib import Path
import json
import subprocess
import sys
from count import main,accessor
import overlay


def zend_map(folder, args, read):
    php=subprocess.check_output([args.php_config,'--php-binary'],text=True).strip()
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


def body(text, signature):
    return accessor(text, signature) if signature in text else RUST_GET_CALLBACK


if __name__=='__main__':
    sys.exit(main(signatures=['static PHP_METHOD(swoole_server_port, getCallback) {'],
                  witness=Path(__file__).with_suffix('.cc'),scope=__doc__,
                  source_path='ext-src/swoole_server_port.cc',extra_support=zend_map,body_extractor=body))
