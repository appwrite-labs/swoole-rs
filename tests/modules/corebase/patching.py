"""The hybrid build's base.cc and actual-call proof for corebase correctness gates."""
import os
from pathlib import Path
import platform
import re
import shutil
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import hybrid_tree

CALLS = ('type_size', 'gcd', 'lcm', 'parse_stoi', 'systemd_listen_fds_action', 'itoa', 'radix', 'hex_mode', 'version', 'dirname', 'mkdir_next', 'lifecycle_next', 'hook_add_action')


def check_git_environment():
    redirected = {'GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_INDEX_FILE',
                  'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES',
                  'GIT_CEILING_DIRECTORIES', 'GIT_CONFIG_COUNT', 'GIT_CONFIG_PARAMETERS'}
    injected = sorted(key for key in os.environ if key in redirected
                      or key.startswith(('GIT_CONFIG_KEY_', 'GIT_CONFIG_VALUE_')))
    if injected:
        raise ValueError('Git environment requires a separate protocol: ' + ', '.join(injected))


def hybrid_corebase(archive):
    """base.cc of the hybrid build the archive came from, the flags its build compiled it with
    (its patched headers, Rust headers and config.h) and a receipt."""
    source, flags = hybrid_tree(archive)
    patched = (source / 'src/core/base.cc').read_bytes()
    if any(('swr_v1_corebase_' + name + '(').encode() not in patched for name in CALLS):
        raise ValueError('Required corebase caller absent')
    if b'swr_v1_core_init()' not in patched:
        raise ValueError('swoole_init does not take its guard and defaults from the Rust core record')
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
