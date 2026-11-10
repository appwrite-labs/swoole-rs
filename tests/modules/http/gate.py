"""Shared helpers for the HTTP differential gates: body extraction, the two compile sides
(pin, and the Cargo build SDK's headers), the optional ASan HTTP archive and the run/report step."""
import io
import json
import os
import shlex
from pathlib import Path
import subprocess
import sys
import tarfile
sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, run_isolated, write_report
from tools.build_ext import native_flags
from tools.build_upstream import COMMIT as PIN
def body(source, signature):
    """The definition starting at signature. Braces count only in the first branch of
    each #if/#else, so alternatives such as `#if A switch (x) { #else switch (y) { #endif`
    are counted once."""
    start = source.index(signature); position = source.index('{', start); depth = 0; branches = []
    for line in source[position:].splitlines(keepends=True):
        directive = line.strip()
        if directive.startswith('#if'): branches.append(False)
        elif directive.startswith(('#else', '#elif')) and branches: branches[-1] = True
        elif directive.startswith('#endif') and branches: branches.pop()
        elif not any(branches):
            for offset, char in enumerate(line):
                depth += (char == '{') - (char == '}')
                if depth == 0 and char == '}':
                    return source[start:position + offset + 1]
        position += len(line)
    raise ValueError('Unterminated definition: ' + signature)
def add_archive_arguments(parser):
    parser.add_argument('--archive', type=Path, required=True, help='Rust archive of the hybrid build under test')
    parser.add_argument('--rust-sanitize', action='store_true', help='Link a private ASan HTTP Rust archive instead; precompiled std remains uninstrumented')


def prepare_archive(args, tree, report):
    """The archive to link: the hybrid build's, or with --rust-sanitize a private ASan HTTP one."""
    if not args.rust_sanitize:
        return args.archive.resolve()
    # Cargo instruments the HTTP crate and every actual dependency, including
    # the canonical proxy/string owner; no hand-maintained extern inventory.
    target = tree/'rust-asan'
    archive = target/'release/libswoole_rs_http.a'
    command = ['cargo', 'rustc', '--locked', '--manifest-path', str(ROOT/'Cargo.toml'),
               '--release', '-p', 'swoole-rs-http', '--lib', '--crate-type', 'staticlib',
               '--target-dir', str(target)]
    compiled = run_isolated(command, timeout=120, env={**os.environ, 'RUSTC_BOOTSTRAP': '1',
        'RUSTFLAGS': '-Zsanitizer=address -Cpanic=abort'})
    report['rust_compile'] = {'command': command, 'returncode': compiled.returncode,
                             'stdout': compiled.stdout, 'stderr': compiled.stderr}
    return archive if compiled.returncode == 0 and archive.is_file() else None



SANITIZE = ['-O1', '-g', '-fsanitize=address,undefined', '-fno-sanitize-recover=all']


def openssl(option):
    return shlex.split(subprocess.check_output(['pkg-config', option, 'openssl'], text=True))


class Sides:
    """A gate's two sides. The upstream side sees only the pin: its include/ and `paths`,
    extracted under tree/pin. The candidate side is the SDK the archive came from: its
    headers and switches (native_flags). Both also see the gate's oracles and the files it
    generates under tree. Drivers compile with the candidate's flags."""

    def __init__(self, source, archive, tree, *paths, sanitize=True):
        self.tree, self.pin = tree, tree/'pin'
        self.sanitize = SANITIZE if sanitize else ['-O1', '-g']
        data = subprocess.check_output(['git', '-C', str(source), 'archive', PIN, 'include', *paths])
        with tarfile.open(fileobj=io.BytesIO(data)) as members:
            for member in members:
                if member.isfile():
                    path = self.pin/member.name; path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(members.extractfile(member).read())
        hybrid = native_flags(archive)
        # Rust ABI headers: declarations only, the same for both sides.
        self.abi = Path(hybrid[-1][2:])
        common = ['-std=c++17', *self.sanitize, '-I'+str(Path(__file__).resolve().parent), '-I'+str(tree), *openssl('--cflags')]
        self.flags = {'upstream': ['-I'+str(self.pin/'include'), '-I'+str(self.pin), '-I'+str(self.abi), *common],
                      'candidate': [*hybrid, *common]}

    def original(self, path):
        return (self.pin/path).read_text()

    def build(self, binary, units, archive, report, gc_sections=False):
        """Compile each (side, source) unit with its side's flags, then link them with the archive.
        gc_sections drops the archive's unreached references to C++ (the server state's)."""
        objects = []
        for side, source in units:
            objects.append(self.tree/(Path(source).name+'.o'))
            command = ['c++', *self.flags[side], '-c', str(source), '-o', str(objects[-1])]
            result = run_isolated(command, timeout=60)
            report.setdefault('compile', []).append({'command': command, 'returncode': result.returncode, 'stderr': result.stderr})
            if result.returncode:
                print(result.stderr, file=sys.stderr); return False
        command = ['c++', *self.sanitize, *map(str, objects), str(archive), '-lm', *openssl('--libs'), '-o', str(binary)]
        if sys.platform.startswith('linux'): command += ['-ldl', '-lpthread', '-lrt', '-lutil', *(['-Wl,--gc-sections'] if gc_sections else [])]
        result = run_isolated(command, timeout=60)
        report['compile'].append({'command': command, 'returncode': result.returncode, 'stderr': result.stderr})
        if result.returncode:
            print(result.stderr, file=sys.stderr)
        return not result.returncode


def run(binary, symbols, output, report, timeout=30):
    """Require the Rust symbols in the gate binary, run it and record its last JSON line."""
    present = subprocess.check_output(['nm', '-g', str(binary)], text=True)
    assert all(name in present for name in symbols)
    try:
        result = run_isolated([str(binary)], timeout=timeout)
    except subprocess.TimeoutExpired as expired:
        # communicate() hands back the partial output as bytes, even in text mode.
        text = lambda data: data.decode(errors='replace') if isinstance(data, bytes) else data or ''
        result = subprocess.CompletedProcess(expired.cmd, None, text(expired.stdout), text(expired.stderr) + f'\nTimed out after {timeout} s\n')
    report['run'] = {'returncode': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr, 'timeout': timeout}
    if result.returncode != 0: report['passed'] = False; write_report(output, report); print(result.stderr, file=sys.stderr); return 1
    report['outcome'] = json.loads(result.stdout.splitlines()[-1]); report['passed'] = True
    write_report(output, report); print(json.dumps(report['outcome']))
    return 0

