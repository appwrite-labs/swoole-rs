#!/usr/bin/env python3
"""Run unchanged File.open_twice assertions and C++ File lifetime/IO callers against the pin and the selected native headers."""
import argparse
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_ext import native_flags
from tools.build_upstream import COMMIT
from tools.harness import run_isolated


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['source', 'archive', 'output']:
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    report = dict(passed=False, pin=COMMIT, commands=[], original_cases=['file.open_twice'])
    def pinned(name):
        return subprocess.check_output(['git', '-C', str(args.source), 'show', COMMIT + ':' + name])
    try:
        flags = native_flags(args.archive)
        original = pinned('core-tests/src/os/file.cpp').decode()
        start = original.index('TEST(file, open_twice)'); opening = original.index('{', start)
        end, depth = opening + 1, 1
        while depth:
            depth += (original[end] == '{') - (original[end] == '}'); end += 1
        # Keep the original test body and every assertion byte unchanged.
        driver = '''#include "swoole_socket.h"
#include <cassert>
using namespace swoole;
#define TEST(group,name) void name()
#define ASSERT_TRUE(x) assert(x)
#define ASSERT_FALSE(x) assert(!(x))
''' + original[start:end] + '''
int main(int argc, char **argv) {
 assert(argc == 2); open_twice();
 const std::string path = std::string(argv[1]) + "/owner";
 File file(path, File::RW | File::CREATE, 0600);
 assert(file.ready());
 const auto *view = &file.get_path(); assert(*view == path);
 assert(file.write(std::string("abc")) == 3);
 assert(file.get_offset() == 3); assert(file.set_offset(0) == 0);
 char bytes[4]{}; assert(file.read(bytes, 3) == 3); assert(std::string(bytes, 3) == "abc");
 assert(file.pwrite("z", 1, 1) == 1); assert(file.pread(bytes, 3, 0) == 3);
 assert(std::string(bytes, 3) == "azc"); assert(file.get_offset() == 3);
 assert(file.truncate(2)); assert(file.sync()); assert(file.lock(LOCK_EX)); assert(file.unlock());
 File copied(file); assert(copied.get_fd() == file.get_fd()); assert(copied.get_path() == path);
 copied.release(); assert(!copied.ready());
 File assigned(-1); assigned = file; assert(assigned.get_fd() == file.get_fd());
 assert(assigned.get_path() == path); assigned.release(); const File *same = &assigned; assigned = *same; assert(!assigned.ready());
 assert(file.open(file.get_path(), File::READ)); assert(&file.get_path() == view); assert(*view == path);
 assert(file.read(bytes, 3) == 2); assert(std::string(bytes, 2) == "az");
 const int fd = file.get_fd(); file.release(); assert(!file.ready()); assert(!file.close());
 assert(fcntl(fd, F_GETFD) >= 0); assert(::close(fd) == 0);

 assert(File::exists(path)); assert(File::remove(path)); assert(!File::exists(path));
}
'''
        with tempfile.TemporaryDirectory(prefix='file-owner-', dir=args.output.parent) as directory:
            work = Path(directory); native = work / 'native'; native.mkdir()
            headers = subprocess.check_output(['git', '-C', str(args.source), 'archive', COMMIT, 'include'])
            with tarfile.open(fileobj=io.BytesIO(headers)) as archive:
                for member in archive:
                    if member.isfile():
                        path = native / member.name; path.parent.mkdir(parents=True, exist_ok=True)
                        path.write_bytes(archive.extractfile(member).read())
            source = work / 'driver.cc'; source.write_text(driver)
            original_file = work / 'file.cc'; original_file.write_bytes(pinned('src/os/file.cc'))
            link = ['-Wl,-dead_strip'] if sys.platform == 'darwin' else ['-Wl,--gc-sections', '-ldl', '-lpthread', '-lm']
            for engine in ['native', 'rust']:
                binary = work / engine / 'consumer'; binary.parent.mkdir(exist_ok=True)
                compile = ['c++', '-std=c++14', '-O2', '-Wall', '-Wextra', '-Werror', '-Wno-unused-function',
                           '-ffunction-sections', '-fdata-sections', *(flags if engine == 'rust' else ['-I' + str(native / 'include')]), *subprocess.check_output(['pkg-config', '--cflags', 'openssl'], text=True).split(), str(source)]
                compile += [str(args.archive.resolve())] if engine == 'rust' else [str(original_file)]
                compile += [*link, '-o', str(binary)]
                for command in [compile, [str(binary), str(binary.parent)]]:
                    result = run_isolated(command, cwd=work, timeout=60)
                    report['commands'].append(dict(engine=engine, command=command, exit=result.returncode, stderr=result.stderr))
                    if result.returncode or result.stderr:
                        raise RuntimeError(engine + ' File caller failed')
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key: report.get(key) for key in ['passed', 'original_cases', 'error']}))
    return not report['passed']


if __name__ == '__main__':
    raise SystemExit(main())
