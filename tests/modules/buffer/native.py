"""Shared steps of the native buffer gates: compile, then run."""
import sys

from tools.harness import ROOT, run_compiler, run_isolated

SOURCE = ROOT / 'target/upstream/src'
LINK = ['-Wl,-dead_strip' if sys.platform == 'darwin' else '-Wl,--gc-sections', '-lm'] + (
    ['-ldl', '-lpthread', '-lrt', '-lutil'] if sys.platform.startswith('linux') else [])


def build_and_run(command, binary, *arguments, timeout=60):
    compiled = run_compiler([*map(str, command), '-o', str(binary)], timeout=60)
    assert compiled['returncode'] == 0, compiled['stderr']
    return run_isolated([str(binary), *arguments], timeout=timeout)

