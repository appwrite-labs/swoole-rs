"""The hybrid build's corebase glue (native/corebase/globals.cc) and actual-call proof for corebase correctness gates."""
import os
from pathlib import Path
import platform
import re
import shutil
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import hybrid_tree

CALLS = ('parse_stoi',)
# Upstream functions Rust defines under their own C++ symbols; the hybrid tree has no body.
DEFINED = ('_Z21swoole_common_divisorjj', '_Z22swoole_common_multiplejj', '_Z22swoole_version_comparePKcS0_',
           '_Z14swoole_dec2hexmi', '_Z16swoole_type_sizec', '_Z11swoole_itoaPcl', '_Z14swoole_hex2decPKcPm')


def check_git_environment():
    redirected = {'GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_INDEX_FILE',
                  'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES',
                  'GIT_CEILING_DIRECTORIES', 'GIT_CONFIG_COUNT', 'GIT_CONFIG_PARAMETERS'}
    injected = sorted(key for key in os.environ if key in redirected
                      or key.startswith(('GIT_CONFIG_KEY_', 'GIT_CONFIG_VALUE_')))
    if injected:
        raise ValueError('Git environment requires a separate protocol: ' + ', '.join(injected))


def hybrid_corebase(archive):
    """globals.cc as the hybrid build the archive came from staged it (what is left of base.cc
    and misc.cc), the flags its build compiled it with (its patched headers, Rust headers and
    config.h) and a receipt. error.cc rides along: the log host globals.cc registers at load
    calls its swoole_strerror and swoole_is_ignored_error."""
    source, flags = hybrid_tree(archive)
    patched = (source / 'src/swoole_rs_corebase_globals.cc').read_bytes()
    if any(('swr_v1_corebase_' + name + '(').encode() not in patched for name in CALLS):
        raise ValueError('Required corebase caller absent')
    if b'swr_v1_core_init()' not in patched:
        raise ValueError('swoole_init does not take its guard and defaults from the Rust core record')
    patched += b'\n' + (source / 'src/core/error.cc').read_bytes()
    return patched, [*flags, '-DHAVE_CONFIG_H', '-I' + str(source)], {
        'hybrid_tree': str(source), 'required_callers': [*CALLS, 'core_init']}


def prove_calls(binary, names, selected, run, prefix):
    nm = run(['nm', '-g', binary])
    disassembly = run(['otool', '-tvV', binary] if platform.system() == 'Darwin' else ['objdump', '-d', '-C', binary])
    text = disassembly.decode(errors='replace')
    calls = {name: bool(re.search(r'\b(?:bl|callq?|b|jmpq?)\s+[^\n]*\b_?swr_v1_corebase_' + name + r'\b', text)) for name in names}
    definitions = {name: bool(re.search(rb'\b[Tt] _?swr_v1_corebase_' + name.encode() + rb'\s*$', nm, re.M)) for name in names}
    Path(str(prefix) + '.nm').write_bytes(nm)
    Path(str(prefix) + '.asm').write_bytes(disassembly)
    shutil.copy2(binary, str(prefix) + '.bin')
    if selected and (not all(calls.values()) or not all(definitions.values())):
        raise ValueError('Selected binary lacks actual corebase Rust calls/definitions')
    if not selected and b'swr_v1_corebase_' in nm:
        raise ValueError('Original comparator unexpectedly contains corebase Rust symbols')
    return {'calls': calls, 'definitions': definitions}


def prove_definitions(obj, binary, selected, run):
    """The original base.o defines each DEFINED symbol; the hybrid base.o leaves it to the
    Rust archive, so the linked binary defines it."""
    def defined(path):
        listing = run(['nm', '-g', path]).decode(errors='replace')
        return {name for name in DEFINED if re.search(r'\b[Tt] _?' + re.escape(name) + r'\s*$', listing, re.M)}
    own = defined(obj)
    rust = defined(binary) if selected else set()
    if selected and (own or rust != set(DEFINED)):
        raise ValueError('The Rust archive must define these, and the hybrid base.cc must not: '
                         + ', '.join(sorted((set(DEFINED) - rust) | own)))
    if not selected and own != set(DEFINED):
        raise ValueError('Original base.cc lacks an upstream definition: ' + ', '.join(sorted(set(DEFINED) - own)))
    return {'base_defines': sorted(own), 'rust_defines': sorted(rust)}
