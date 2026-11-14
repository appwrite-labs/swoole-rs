"""Shared steps of the native buffer gates: compile against the pinned tree or the Cargo build's tree, run."""
import json
import sys

from tools.build_ext import native_flags
from tools.harness import ROOT, run_compiler, run_isolated

SOURCE = ROOT / 'target/upstream/src'
INCLUDES = ['-I' + str(ROOT / 'native/include'), '-I' + str(ROOT / 'native/buffer')]
LINK = ['-Wl,-dead_strip' if sys.platform == 'darwin' else '-Wl,--gc-sections', '-lm'] + (
    ['-ldl', '-lpthread', '-lrt', '-lutil'] if sys.platform.startswith('linux') else [])
SANITIZE = ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']


def build_and_run(command, binary, *arguments, timeout=60):
    compiled = run_compiler([*map(str, command), '-o', str(binary)], timeout=60)
    assert compiled['returncode'] == 0, compiled['stderr']
    return run_isolated([str(binary), *arguments], timeout=timeout)


def native_and_selected(tree, test_source, archive, sanitize, *defines):
    """Outcome JSON of one test compiled against pinned buffer.cc, then against the Cargo build's
    headers with its Rust archive, which defines Buffer."""
    flags = native_flags(archive)
    outcomes = {}
    for name in ['native', 'selected']:
        headers = ['-I' + str(tree / 'include'), *INCLUDES] if name == 'native' else ['-DSWOOLE_RS_BUFFER', *flags]
        command = ['c++', '-std=c++17', '-O1', '-g', *defines, '-ffunction-sections', '-fdata-sections',
                   *headers, *([tree / 'src/core/buffer.cc'] if name == 'native' else []), test_source]
        if name == 'selected':
            command.append(archive)
        command += LINK + (SANITIZE if sanitize else [])
        result = build_and_run(command, tree / name)
        assert result.returncode == 0 and not result.stderr, f'{name}: {result.stdout}{result.stderr}'
        outcomes[name] = json.loads(result.stdout)
    return outcomes
