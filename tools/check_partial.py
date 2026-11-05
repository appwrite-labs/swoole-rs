#!/usr/bin/env python3
"""Syntax-check shared-header consumers in partial trees, reusing a configured full build."""
import argparse
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.build_hybrid import GRADUATED, MODULES, ROOT, apply_patch_verified, patches_for
from tools.build_upstream import COMMIT
from tools.harness import write_report


def shared_header(name):
    return name.endswith('.h') and (name.startswith(('include/', 'native/')) or
                                   name.startswith('ext-src/php_swoole_'))


def changed_headers(base):
    base = subprocess.check_output(['git', 'merge-base', base, 'HEAD'], cwd=ROOT, text=True).strip()
    names = subprocess.check_output(['git', 'diff', '--name-only', base, 'HEAD'], cwd=ROOT, text=True).splitlines()
    headers = {name for name in names if shared_header(name)}
    if set(names) & {'tools/check_partial.py', '.github/workflows/pr.yml'}:
        headers.update({'include/swoole.h', 'ext-src/php_swoole_private.h', 'ext-src/php_swoole_cxx.h'})
    for name in names:
        if not name.startswith('migration/patches/') or not name.endswith('.patch'):
            continue
        # Both versions matter: removing a patch can remove a declaration too.
        for revision in (base, 'HEAD'):
            body = subprocess.run(['git', 'show', f'{revision}:{name}'], cwd=ROOT, capture_output=True, text=True)
            for line in body.stdout.splitlines():
                if line.startswith('diff --git '):
                    target = shlex.split(line)[3][2:]
                    if shared_header(target):
                        headers.add(target)
    return headers


def consumers(source, headers, units):
    dependents = defaultdict(set)
    for directory in ('src', 'ext-src', 'include'):
        for path in (source / directory).rglob('*'):
            if path.suffix in {'.c', '.cc', '.cpp', '.h', '.hpp'}:
                for include in re.findall(r'^\s*#\s*include\s*[<"]([^>"\n]+)', path.read_text(), re.M):
                    dependents[Path(include).name].add(path.relative_to(source).as_posix())
    pending, found = list(headers), set()
    while pending:
        for name in dependents[Path(pending.pop()).name] - found:
            found.add(name)
            pending.append(name)
    return sorted(found & set(units))


def syntax_command(line):
    args = shlex.split(line)
    args = args[args.index('--mode=compile') + 1:]
    result = []
    skip = False
    for arg in args:
        if skip:
            skip = False
        elif arg in {'-o', '-MF', '-MT', '-MQ'}:
            skip = True
        elif arg not in {'-c', '-MMD', '-MD'}:
            result.append(arg)
    return [*result, '-fsyntax-only']


def check_profile(name, modules, record, configured, headers, output):
    work = ROOT / 'target/partial-syntax' / name
    shutil.rmtree(work, ignore_errors=True)
    work.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(['git', 'clone', '-q', '--shared', '--no-checkout', str(configured), str(work)], check=True)
    subprocess.run(['git', '-C', str(work), 'checkout', '-q', '--detach', COMMIT], check=True)
    selected = {'M00', *(MODULES[module][0] for module in modules)}
    plan = json.loads((ROOT / 'migration/modules.json').read_text())
    for patch in patches_for([module for module in plan['modules'] if module['id'] in selected]):
        apply_patch_verified(work, ROOT / patch['path'], patch['upstream_paths'], patch['sha256'])
    # Configure against the existing full prefix; syntax checks need headers, not new archives.
    configure = [arg for arg in record['configure'] if not arg.startswith('--enable-swoole-rs-')]
    configure += ['--enable-swoole-rs-' + module for module in sorted(set(modules) - set(GRADUATED))]
    log = output / (name + '.log')
    env = os.environ.copy()
    env.update({key: value for key, value in record['inputs']['environment'].items() if value is not None})
    env['CPPFLAGS'] = record['inputs']['effective_cppflags']
    with log.open('w') as stream:
        for command in ([record['inputs']['phpize']], configure):
            subprocess.run(command, cwd=work, env=env, stdout=stream, stderr=subprocess.STDOUT, check=True)
        makefile = (work / 'Makefile').read_text()
        objects = re.findall(r'^([^\s:]+\.lo):', makefile, re.M)
        units = {str(Path(obj).with_suffix('.cc')): obj for obj in objects if (work / Path(obj).with_suffix('.cc')).is_file()}
        units.update({str(Path(obj).with_suffix('.c')): obj for obj in objects if (work / Path(obj).with_suffix('.c')).is_file()})
        selected_units = consumers(work, headers, units)
        if not selected_units:
            return {'profile': name, 'modules': sorted(modules), 'units': [], 'log': str(log)}
        dry = subprocess.check_output(['make', '-n', *(units[unit] for unit in selected_units)], cwd=work, text=True)
        commands = [syntax_command(line) for line in dry.splitlines() if '--mode=compile ' in line]
        if len(commands) != len(selected_units):
            raise ValueError(f'{name}: missing compiler commands for selected consumers')
        def compile_unit(command):
            return subprocess.run(command, cwd=work, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        with ThreadPoolExecutor(max_workers=min(4, os.cpu_count() or 1)) as pool:
            for command, result in zip(commands, pool.map(compile_unit, commands)):
                print(shlex.join(command), file=stream)
                stream.write(result.stdout)
                stream.flush()
                result.check_returncode()
    return {'profile': name, 'modules': sorted(modules), 'units': selected_units, 'log': str(log)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', required=True, help='PR base ref; compares its merge base to HEAD')
    parser.add_argument('--manifest', type=Path, help='Use this existing all-modules build manifest; otherwise require one unique cached full build')
    args = parser.parse_args()
    headers = changed_headers(args.base)
    if not headers:
        print('No shared header changed; partial syntax checks not needed.')
        return 0
    manifests = ([args.manifest.resolve(strict=True)] if args.manifest else
                 [path for path in (ROOT / 'target/hybrid/builds').glob('*/build-manifest.json')
                  if set(json.loads(path.read_text())['inputs']['modules']) == set(MODULES)])
    if len(manifests) != 1:
        parser.error('Expected one configured all-modules build; select its --manifest when several exist')
    record = json.loads(manifests[0].read_text())
    if set(record['inputs']['modules']) != set(MODULES):
        parser.error('--manifest must name an all-modules build')
    configured = manifests[0].parent / 'src'
    output = ROOT / 'target/reports/partial-syntax'
    output.mkdir(parents=True, exist_ok=True)
    report = {'headers': sorted(headers), 'profiles': [], 'passed': False}
    try:
        for name, modules in [('graduated', set(GRADUATED)), ('without-clients', set(MODULES) - {'clients'})]:
            print(f'Checking {name} consumers of {", ".join(sorted(headers))}', flush=True)
            try:
                report['profiles'].append(check_profile(name, modules, record, configured, headers, output))
            except subprocess.CalledProcessError as error:
                report['error'] = str(error)
                log = output / (name + '.log')
                if log.exists():
                    print('\n'.join(log.read_text().splitlines()[-40:]), file=sys.stderr)
                raise
        report['passed'] = True
    finally:
        write_report(output / 'report.json', report)
    return 0


if __name__ == '__main__':
    sys.exit(main())
