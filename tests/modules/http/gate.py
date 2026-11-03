#!/usr/bin/env python3
"""Compare actual pinned/patched HTTP helper bodies, without servers or parsers."""
import argparse
import hashlib
import io
import json
import os
import shlex
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, run_isolated, write_report
from tools.build_hybrid import hybrid_tree
from tools.build_upstream import COMMIT as PIN
ORIGINAL_TESTS = ['EncodeDecodeBasic', 'EncodeDecodeWithSpecialChars', 'get_method', 'get_method_str', 'parse_multipart_boundary']
FUNCTIONS = ['const char *get_status_message(', 'bool parse_multipart_boundary(', 'size_t url_decode(', 'char *url_encode(', 'int get_method(', 'const char *get_method_string(']
def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()
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
def extract(source, namespace):
    parts = ['static thread_local char http_status_message[128];']
    if 'static const char *method_strings[]' in source:
        array_start = source.index('static const char *method_strings[]'); array_end = source.index('};', array_start) + 2
        parts.insert(0, source[array_start:array_end])
    if 'static int url_htoi(' in source: parts.append(body(source, 'static int url_htoi('))
    parts += [body(source, signature) for signature in FUNCTIONS]
    return '#include "oracle.h"\nnamespace ' + namespace + ' {\n' + '\n'.join(parts) + '\n}\n'
def original_assertions(tests):
    adapter = r'''#define ASSERT_TRUE(value) do { if (!(value)) throw std::runtime_error("Original assertion: " #value); } while (false)
#define ASSERT_FALSE(value) ASSERT_TRUE(!(value))
#define ASSERT_EQ(a, b) ASSERT_TRUE((a) == (b))
#define EXPECT_EQ(a, b) ASSERT_EQ(a, b)
#define EXPECT_STREQ(a, b) ASSERT_TRUE(std::strcmp(a, b) == 0)
#define ASSERT_STREQ(a, b) EXPECT_STREQ(a, b)
#define TEST(group, name) static void original_##name()
'''
    output = '#include "oracle.h"\n' + adapter
    bodies = [body(tests, 'TEST(http_server, ' + name + ')') for name in ORIGINAL_TESTS]
    for engine in ['upstream', 'candidate']:
        output += 'namespace ' + engine + '_assertions {\nnamespace swoole { namespace http_server {\n'
        output += '\n'.join('using ::' + engine + '::' + signature.split('(')[0].split()[-1].lstrip('*') + ';' for signature in FUNCTIONS) + '\n}}\n'
        output += 'using namespace swoole::http_server;\n' + '\n'.join(bodies) + '\n}\n'
    output += 'void run_original_assertions() {\n'
    for engine in ['upstream', 'candidate']:
        output += '\n'.join(engine + '_assertions::original_' + name + '();' for name in ORIGINAL_TESTS) + '\n'
    return output + '}\n'
def add_archive_arguments(parser):
    parser.add_argument('--archive', type=Path, required=True, help='Rust archive of the hybrid build under test')
    parser.add_argument('--rust-sanitize', action='store_true', help='Link a private ASan HTTP Rust archive instead; precompiled std remains uninstrumented')


def prepare_archive(args, tree, report):
    """The archive to link: the hybrid build's, or with --rust-sanitize a private ASan HTTP one."""
    if not args.rust_sanitize:
        return args.archive.resolve()
    archive = tree/'libhttp-asan.a'
    codec = tree/'libswoole_rs_base64.rlib'
    codec_source = ROOT/'crates/base64/src/lib.rs'
    codec_command = ['rustc', '--edition=2024', '-Zsanitizer=address', '--crate-name=swoole_rs_base64', '--crate-type=rlib', '-C', 'opt-level=3', '-C', 'panic=abort', str(codec_source), '-o', str(codec)]
    compiled = run_isolated(codec_command, timeout=60, env={**os.environ, 'RUSTC_BOOTSTRAP': '1'})
    report['rust_codec_compile'] = {'command': codec_command, 'returncode': compiled.returncode, 'stdout': compiled.stdout, 'stderr': compiled.stderr}
    if compiled.returncode:
        return None
    command = ['rustc', '--edition=2024', '-Zsanitizer=address', '--extern', 'swoole_rs_base64='+str(codec), '--crate-type=staticlib', '-C', 'opt-level=3', '-C', 'panic=abort', str(ROOT/'crates/http/src/lib.rs'), '-o', str(archive)]
    compiled = run_isolated(command, timeout=60, env={**os.environ, 'RUSTC_BOOTSTRAP': '1'})
    report['rust_compile'] = {'command': command, 'returncode': compiled.returncode, 'stdout': compiled.stdout, 'stderr': compiled.stderr}
    return None if compiled.returncode else archive


SANITIZE = ['-O1', '-g', '-fsanitize=address,undefined', '-fno-sanitize-recover=all']


def openssl(option):
    return shlex.split(subprocess.check_output(['pkg-config', option, 'openssl'], text=True))


class Sides:
    """A gate's two sides. The upstream side sees only the pin: its include/ and `paths`,
    extracted under tree/pin. The candidate side is the hybrid build the archive came from:
    its fully patched tree and flags. Both also see the gate's oracles and the files it
    generates under tree. Drivers compile with the candidate's flags."""

    def __init__(self, source, archive, tree, *paths, sanitize=True):
        self.tree, self.pin = tree, tree/'pin'
        self.sanitize = SANITIZE if sanitize else ['-O1', '-g']
        data = subprocess.check_output(['git', '-C', str(source), 'archive', PIN, 'include', *paths])
        with tarfile.open(fileobj=io.BytesIO(data)) as members:
            for member in members:
                if member.isfile():
                    path = self.pin/member.name; path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(members.extractfile(member).read())
        self.hybrid, hybrid = hybrid_tree(archive)
        # Rust ABI headers: declarations only, the same for both sides.
        self.abi = Path(next(flag for flag in hybrid if flag.endswith('rust-prefix/include'))[2:])
        common = ['-std=c++17', *self.sanitize, '-I'+str(Path(__file__).resolve().parent), '-I'+str(tree), *openssl('--cflags')]
        self.flags = {'upstream': ['-I'+str(self.pin/'include'), '-I'+str(self.pin), '-I'+str(self.abi), *common],
                      'candidate': [*hybrid, '-I'+str(self.hybrid), *common]}

    def original(self, path):
        return (self.pin/path).read_text()

    def selected(self, path):
        return (self.hybrid/path).read_text()

    def native(self, report, *paths):
        """Compile the hybrid tree's whole native translation units as C++14."""
        flags = ['-std=c++14' if flag == '-std=c++17' else flag for flag in self.flags['candidate']]
        report['native_cxx_standard'], report['shim_cxx_standard'] = 'c++14', 'c++17'
        for path in paths:
            command = ['c++', *flags, '-c', str(self.hybrid/path), '-o', str(self.tree/(Path(path).stem+'.native.o'))]
            result = run_isolated(command, timeout=60)
            report.setdefault('whole_native_translation_units', {})[path] = {'command': command, 'returncode': result.returncode, 'stderr': result.stderr}
            if result.returncode:
                print(result.stderr, file=sys.stderr); return False
        return True

    def build(self, binary, units, archive, report):
        """Compile each (side, source) unit with its side's flags, then link them with the archive."""
        objects = []
        for side, source in units:
            objects.append(self.tree/(Path(source).name+'.o'))
            command = ['c++', *self.flags[side], '-c', str(source), '-o', str(objects[-1])]
            result = run_isolated(command, timeout=60)
            report.setdefault('compile', []).append({'command': command, 'returncode': result.returncode, 'stderr': result.stderr})
            if result.returncode:
                print(result.stderr, file=sys.stderr); return False
        command = ['c++', *self.sanitize, *map(str, objects), str(archive), '-lm', *openssl('--libs'), '-o', str(binary)]
        if sys.platform.startswith('linux'): command += ['-ldl', '-lpthread', '-lrt', '-lutil']
        result = run_isolated(command, timeout=60)
        report['compile'].append({'command': command, 'returncode': result.returncode, 'stderr': result.stderr})
        if result.returncode:
            print(result.stderr, file=sys.stderr)
        return not result.returncode


def run(binary, symbols, output, report, timeout=30):
    """Require the Rust symbols in the gate binary, run it and record its last JSON line."""
    present = subprocess.check_output(['nm', '-g', str(binary)], text=True)
    assert all(name in present for name in symbols)
    result = run_isolated([str(binary)], timeout=timeout)
    report['run'] = {'returncode': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}
    if result.returncode: write_report(output, report); print(result.stderr, file=sys.stderr); return 1
    report['outcome'] = json.loads(result.stdout.splitlines()[-1]); report['passed'] = True
    write_report(output, report); print(json.dumps(report['outcome']))
    return 0


def prepare_index_buffer(sides):
    """Each side's native String facade, with the format_impl of that side's own swoole_string.h
    and the String::format_step it calls, if any, from that side's string.cc."""
    output = '#pragma once\n'
    for namespace, root in [('upstream_context', sides.pin), ('candidate_context', sides.hybrid)]:
        template = body((root/'include/swoole_string.h').read_text(), 'size_t format_impl(')
        members = ''
        if 'format_step(' in template:
            string = (root/'src/core/string.cc').read_text()
            members = 'static '+body(string, 'void check_string_status(')+'\n'+body(string, 'swr_v1_string_format_step_result String::format_step(').replace('String::format_step', 'format_step')+'\n'
            output += '#include "swoole_rs_string.h"\n'
        output += 'namespace '+namespace+' {\nclass IndexBuffer { public:\n std::vector<unsigned char> storage;char *str=nullptr;size_t size=0,length=6,offset=7;unsigned calls=0,fail=0;std::vector<size_t> events;\n enum FormatFlag{FORMAT_APPEND=1,FORMAT_GROW=2};\n explicit IndexBuffer(size_t capacity):storage(capacity,0xa5),size(capacity){str=reinterpret_cast<char*>(storage.data());if(capacity>=6)memcpy(str,"prefix",6);events.reserve(32);}\n void reserve(size_t requested){events.push_back(requested);if(++calls==fail)throw std::bad_alloc();size_t count=size==0?requested:SW_MEM_ALIGNED_SIZE(requested);storage.resize(count,0xa5);size=count;str=reinterpret_cast<char*>(storage.data());}\n void extend_align(size_t minimum){size_t count=SW_MEM_ALIGNED_SIZE(size*2);while(count<minimum)count*=2;reserve(count);}\n void append(const char *input,size_t count){if(length+count>size)reserve(length+count);memcpy(str+length,input,count);length+=count;}\n'+members+' template <typename... Args>\n'+template+'\n};\n}\n'
    (sides.tree/'index_buffer.h').write_text(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--archive', type=Path, required=True, help='Rust archive of the hybrid build under test')
    parser.add_argument('--output', default='target/reports/http-helpers.json')
    args = parser.parse_args()
    report = {'pin': PIN, 'scope': 'Extracted actual first-party helper bodies; not full native TU, original parser/server tests or PHP parity', 'cpp_sanitized_rust_not_instrumented': True}
    with tempfile.TemporaryDirectory(prefix='swr-http-') as temp:
        tree = Path(temp); path = 'src/protocol/http.cc'
        sides = Sides(args.source, args.archive, tree, 'thirdparty/multipart_parser.h', path)
        tests = subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':core-tests/src/server/http_server.cpp'], text=True)
        assertions = tree / 'original_assertions.cc'; assertions.write_text(original_assertions(tests))
        report['selected_original_tests'] = ORIGINAL_TESTS
        if not sides.native(report, path): write_report(args.output, report); return 1
        for namespace, text in [('upstream', sides.original(path)), ('candidate', sides.selected(path))]:
            (tree / (namespace + '.cc')).write_text(extract(text, namespace))
        binary = tree / 'gate'
        units = [('upstream', tree/'upstream.cc'), ('candidate', tree/'candidate.cc'), ('candidate', Path(__file__).with_name('gate.cc')), ('candidate', assertions)]
        if not sides.build(binary, units, args.archive.resolve(), report): write_report(args.output, report); return 1
        return run(binary, ['swr_v1_http_status', 'swr_v1_http_boundary', 'swr_v1_http_url_encode', 'swr_v1_http_url_decode', 'swr_v1_http_method'], args.output, report)
if __name__ == '__main__': sys.exit(main())
