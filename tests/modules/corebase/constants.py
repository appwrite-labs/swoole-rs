#!/usr/bin/env python3
"""Compare canonical Rust-generated identifiers/defaults with the clean pin, including C++ types and string bytes."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_upstream import COMMIT
from tools.harness import run_isolated


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    report = dict(passed=False, pin=COMMIT, commands=[], profiles=[])
    def original(path):
        return subprocess.check_output(['git', '-C', str(args.source), 'show', COMMIT + ':' + path], text=True)
    try:
        enums = '\n'.join(re.findall(r'enum sw\w+ \{.*?\n\};', original('include/swoole.h') + original('include/swoole_error.h'), re.S))
        names = sorted(set(re.findall(r'^\s*(SW_\w+)\s*(?:=|,)', re.sub(r'/\*.*?\*/', '', enums, flags=re.S), re.M)))
        constants = (ROOT / 'crates/corebase/src/constants.rs').read_text()
        names = sorted(set(names) | set(re.findall(r'pub const (SW_\w+):', constants)))
        prelude = '''#include <cstdint>
#include <cstddef>
#include <iostream>
#include <iomanip>
#include <type_traits>
#include <sys/un.h>
template<class T, bool = std::is_enum<T>::value> struct Signed {static constexpr bool value = std::is_signed<T>::value;};
template<class T> struct Signed<T,true> {static constexpr bool value = std::is_signed<typename std::underlying_type<T>::type>::value;};
template<class T> void emit(const char *name, const T &value) {
 std::cout << name << ':' << sizeof(T) << ':' << Signed<T>::value << ':' << std::is_enum<T>::value << ':' << std::setprecision(30) << static_cast<long double>(value) << '\\n';
}
template<size_t N> void emit(const char *name, const char (&value)[N]) {
 std::cout << name << ':' << N << ':';
 for (size_t i=0;i<N;i++) std::cout << std::hex << static_cast<unsigned>(static_cast<unsigned char>(value[i])) << ',';
 std::cout << std::dec << '\\n';
}
'''
        driver = '\nint main(){\n' + ''.join(f'emit("{name}", {name});\n' for name in names) + '}\n'
        with tempfile.TemporaryDirectory(prefix='constants-', dir=args.output.parent) as directory:
            work = Path(directory)
            (work / 'original_config.h').write_text(original('include/swoole_config.h'))
            for apple in [False, True]:
                platform = '\n#undef __MACH__\n' + ('#define __MACH__ 1\n' if apple else '')
                observations = {}
                for engine in ['native', 'rust']:
                    declarations = ('#include "original_config.h"\n' + enums if engine == 'native' else
                                    '#include "swoole_rs_corebase_constants.h"\n#include "swoole_rs_error.h"')
                    source = work / f'{engine}-{apple}.cc'; binary = source.with_suffix('')
                    source.write_text(prelude + platform + declarations + driver)
                    for command in [['c++', '-std=c++14', '-Wall', '-Wextra', '-Werror', '-I' + str(ROOT / 'native/include'), str(source), '-o', str(binary)], [str(binary)]]:
                        result = run_isolated(command, cwd=work, timeout=60)
                        report['commands'].append(dict(command=command, exit=result.returncode, stderr=result.stderr))
                        if result.returncode or result.stderr:
                            raise RuntimeError(f'{engine} constants comparison failed')
                    observations[engine] = result.stdout.splitlines()
                differences = [dict(native=a, rust=b) for a,b in zip(observations['native'], observations['rust']) if a != b]
                report['profiles'].append(dict(apple=apple, constants=len(names), differences=differences))
                if observations['native'] != observations['rust']:
                    raise RuntimeError('Identifier value/type or literal-byte differences')
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key: report.get(key) for key in ['passed', 'profiles', 'error']}))
    return not report['passed']


if __name__ == '__main__':
    raise SystemExit(main())
