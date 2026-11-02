#!/usr/bin/env python3
"""Normal Port flag and buffer-publication plans; exact non-TLS prefix, no callbacks/reentry/fault/lifetime profiles."""
from pathlib import Path
import sys
from count import main,accessor


# The hybrid moved set() to crates/phpserver; its ListenPort block stays C++ in this host function.
SELECTED_SET='static bool port_apply_settings(ListenPort *port, ServerPortProperty *property, HashTable *vht) {'


def prefix(text, signature):
    start=text.index('    // backlog',text.index(signature if signature in text else SELECTED_SET));end=text.index('    if (port->ssl) {',start)
    return 'bool set_prefix(ListenPort *port, ServerPortProperty *property, HashTable *vht) {\n    zval *ztmp = nullptr;\n'+text[start:end]+'    return true;\n}\n'


def helper(folder,args,read):
    text=read('ext-src/swoole_server_port.cc');p=folder/'apply.inc';signatures=['static void port_apply_number(', 'template<class T> static void port_apply_buffer(', 'static void port_apply_flags('];p.write_text('\n'.join(accessor(text,signature)for signature in signatures if signature in text))
    raw=read('include/swoole.h');start=raw.index('struct DataHead {');end=raw.index('\n};',start)+3
    typedef=next(line for line in raw.splitlines()if 'typedef 'in line and 'SessionId;'in line)
    (folder/'data-head.inc').write_text('namespace swoole {\n'+typedef+'\n'+raw[start:end]+'\n}\n')
    timer=read('include/swoole_timer.h');(folder/'timer-min.inc').write_text(next(line for line in timer.splitlines()if line.startswith('#define SW_TIMER_MIN_SEC '))+'\n')
    (folder/'bounds.inc').write_text('\n'.join(line for line in raw.splitlines()if line.startswith(('#define SW_MAX(', '#define SW_MIN(')))+'\n')
    return {'scope':'Only exact native flag publication helper and each side\'s scalar bound macros; no native objects enter Rust','excluded_native_behaviors':['TLS suffix','callback creation/execution','reentry/destruction','invalid/fatal setting values','floating nonfinite inputs']}


def run(witness, scope):
    return main(signatures=['static PHP_METHOD(swoole_server_port, set) {'],
                  witness=witness,scope=scope,
                  source_path='ext-src/swoole_server_port.cc',body_extractor=prefix,extra_support=helper,
                  support={'config.inc':('include/swoole_config.h',None),
                           'type-size.inc':('src/core/base.cc','int swoole_type_size(char type)')})


if __name__=='__main__':
    sys.exit(run(Path(__file__).with_suffix('.cc'),__doc__))
