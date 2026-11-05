#!/usr/bin/env python3
"""Build pinned native core tests privately; explicit unsupported units never count as passes."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import fnmatch
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import uuid
import shutil
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.build_hybrid import hybrid_tree, module_patches, overlay_hybrid_tree
from tools.build_upstream import COMMIT, require_build_space
from tools.check_migration import validate
from tools.harness import exclusive
from tools.harness import ROOT, run_isolated, write_report


@contextmanager
def timed_phase(report, phase):
    """Concurrent correctness wall time, retained even when a phase raises."""
    started = time.monotonic()
    try:
        yield
    finally:
        timings = report.setdefault('timings_seconds', {})
        timings[phase] = round(timings.get(phase, 0) + time.monotonic() - started, 4)


def run_command(command, cwd, log, timeout, report, checkpoint, phase):
    row = {'command': list(map(str, command)), 'cwd': str(cwd), 'phase': phase,
           'log': str(log), 'timing_basis': 'isolated_wall_clock'}
    started = time.monotonic()
    try:
        result = run_isolated(row['command'], cwd=cwd, timeout=timeout)
        log.parent.mkdir(parents=True, exist_ok=True)
        log.write_text(result.stdout + result.stderr)
        row.update(exit=result.returncode, log_sha256=sha(log))
    except BaseException as error:
        row.update(error=str(error), error_type=type(error).__name__)
        if isinstance(error, subprocess.TimeoutExpired):
            partial = [value.decode(errors='replace') if isinstance(value, bytes) else value or ''
                       for value in (error.stdout, error.stderr)]
            log.parent.mkdir(parents=True, exist_ok=True)
            log.write_text(''.join(partial))
            row.update(timed_out=True, log_sha256=sha(log))
        raise
    finally:
        row['seconds'] = round(time.monotonic() - started, 4)
        report['commands'].append(row)
        timings = report.setdefault('timings_seconds', {})
        timings[phase] = round(timings.get(phase, 0) + row['seconds'], 4)
        checkpoint()
    if result.returncode:
        raise RuntimeError('Native core command failed: ' + str(log))
    return result.stdout


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def hybrid_inputs(archive):
    """What overlay_hybrid_tree copies from the archive's build: its flags and every patched file."""
    source, flags = hybrid_tree(archive)
    names = sorted({name for directory in (ROOT / 'migration/patches').iterdir()
                    for patch in module_patches(directory) for name in patch['upstream_paths']})
    return {'flags': flags, 'files': {name: sha(source / name) if (source / name).exists() else None for name in names}}


def stage(source, destination):
    # core-tests is export-ignored; recover every tracked blob, not git archive.
    entries = subprocess.check_output(['git', '-C', str(source), 'ls-tree', '-r', COMMIT], text=True).splitlines()
    metadata = {}
    for entry in entries:
        prefix, name = entry.split('\t', 1)
        mode, kind, _ = prefix.split()
        if kind != 'blob': raise ValueError('Unsupported non-blob source: ' + name)
        metadata[name] = mode
    names = list(metadata)
    packed = subprocess.check_output(['git', '-C', str(source), 'cat-file', '--batch'],
                                     input=''.join(COMMIT + ':' + n + '\n' for n in names).encode())
    offset = 0
    for name in names:
        end = packed.index(b'\n', offset)
        size = int(packed[offset:end].split()[-1]); offset = end + 1
        path = destination / name
        path.parent.mkdir(parents=True, exist_ok=True)
        data = packed[offset:offset+size]; offset += size + 1
        if path.exists() or path.is_symlink(): path.unlink()
        if metadata[name] == '120000': path.symlink_to(data.decode())
        else:
            path.write_bytes(data)
            path.chmod(0o755 if metadata[name] == '100755' else 0o644)
    return {name: sha(destination / name) for name in names if name.startswith('core-tests/') and metadata[name] != '120000'}


def fixture_binary(binary, tree):
    # Original main derives its fixture root from argv[0]/../, not cwd.
    destination = tree / 'bin/core-tests'
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(binary, destination)
    if sha(binary) != sha(destination):
        raise ValueError('Native fixture launcher differs from built binary')
    return destination


def library_identity(path):
    path = Path(path)
    return {'path': str(path.absolute()), 'resolved_path': str(path.resolve(strict=True)),
            'sha256': sha(path)}


def valid_build_receipt(receipt, binary, library):
    try:
        built = json.loads(receipt.read_text())
        return (built.get('binary_sha256') == sha(binary)
                and built.get('native_library') == library_identity(library))
    except (OSError, ValueError):
        return False


def verify_loader_output(output, expected, system):
    if system == 'Darwin':
        paths = re.findall(r'^dyld\[\d+\]: (?:<[^>]+> )?(/.*libswoole[^/]*\.dylib)\s*$', output, re.M)
    else:
        paths = re.findall(r'^\s*libswoole[^ ]* => (/.*libswoole[^/ ]*) \(0x[0-9a-fA-F]+\)', output, re.M)
    resolved = {str(Path(path).resolve(strict=True)) for path in paths}
    if resolved != {expected['resolved_path']}:
        raise ValueError('Native loader resolved missing or foreign Swoole library: ' + repr(paths))
    if library_identity(expected['path']) != expected:
        raise ValueError('Native library changed during loader verification')
    return {'resolved_paths': sorted(resolved), 'native_library': expected}


def verify_loader(executable, tree, expected, log):
    # Probe only: tracing must never leak into original tests or shell_exec children.
    system = platform.system()
    setting = 'DYLD_PRINT_LIBRARIES' if system == 'Darwin' else 'LD_TRACE_LOADED_OBJECTS'
    command = [str(executable), '--gtest_list_tests']
    result = run_isolated(command, cwd=tree, timeout=30, env={**os.environ, setting: '1'})
    output = result.stdout + result.stderr
    log.write_text(output)
    if result.returncode:
        raise ValueError('Native loader probe failed: ' + str(log))
    return {**verify_loader_output(output, expected, system), 'command': command,
            'environment_override': {setting: '1'}, 'log': str(log), 'log_sha256': sha(log),
            'scope': 'Separate loader probe; original cases run without tracing, with library hashes checked before and after each case'}


def stat_adapter(tree, enabled, system):
    if not enabled:
        return ''
    if system != 'Darwin':
        raise ValueError('Darwin stat field adapter is only valid on Darwin')
    # Pinned HTTP assertions use the Linux spelling for the same timespec member.
    # Adapt this test unit only; production sources and assertion bytes stay pinned.
    return ('\nset_source_files_properties(' + json.dumps(str(tree/'core-tests/src/server/http_server.cpp'))
            + ' PROPERTIES COMPILE_DEFINITIONS "st_mtim=st_mtimespec")\n')


def inventory_from_listing(listing):
    inventory = []
    group = None
    for line in listing.splitlines():
        line = line.split(' #',1)[0].rstrip()
        if line and not line.startswith(' ') and line.endswith('.'):
            group = line.strip()
        elif group and line.startswith('  '):
            inventory.append(group + line.strip())
    if len(set(inventory)) != len(inventory):
        raise ValueError('Duplicate runtime original test identities')
    return inventory


def test_outcome(xml, name, returncode):
    if not xml.exists(): return {'status':'failed','error':'Missing original XML receipt'}
    row = {'status':'failed','xml_sha256':sha(xml)}
    try:
        root = ET.parse(xml)
        cases = list(root.iter('testcase'))
        if len(cases) != 1 or cases[0].get('classname','')+'.'+cases[0].get('name','') != name:
            row['error'] = 'Missing, duplicate or foreign testcase receipt'
        elif list(root.iter('failure')) or list(root.iter('error')) or returncode != 0:
            row['error'] = 'Original assertion or process failed'
        elif cases[0].find('skipped') is not None or cases[0].get('status') == 'notrun':
            row['status'] = 'skipped'
        elif cases[0].get('status') == 'run' and cases[0].get('result') == 'completed':
            row['status'] = 'passed'
        else:
            row['error'] = 'Unknown original execution status'
    except ET.ParseError as error: row['error'] = str(error)
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True, help='Read-only pinned Git object source')
    parser.add_argument('--config', type=Path, required=True, help='Matching configured native config.h')
    parser.add_argument('--archive', type=Path, help="A hybrid build's libswoole_rs_ffi.a: build over that build's all-modules tree")
    parser.add_argument('--exclude-unit', action='append', default=[], help='Explicit incompatible original core-tests/src/*.cpp unit; inventory retained as unsupported, full gate stays failed')
    parser.add_argument('--case', action='append', default=[], help='Exact/glob original testcase, repeatable; no implicit execution')
    parser.add_argument('--cache', type=Path, default=ROOT / 'target/native-core')
    parser.add_argument('--openssl-dir', type=Path, help='Matching native OpenSSL prefix, required for pinned macOS CMake linkage')
    parser.add_argument('--php-dir', default='/usr', help='Prefix containing native php-config')
    parser.add_argument('--ccache', type=Path, default=shutil.which('ccache'), help='Shared bounded compiler cache, detected by default')
    parser.add_argument('--no-ccache', dest='ccache', action='store_const', const=None)
    parser.add_argument('--darwin-stat-adapter', action='store_true', help='Explicit test-unit-only st_mtim=st_mtimespec field spelling adapter for pinned HTTP server originals on Darwin; assertions and production sources unchanged')
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--timeout', type=int, default=90)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.jobs < 1 or args.timeout < 1:
        parser.error('Positive jobs/timeout')
    repository = subprocess.run(['git','rev-parse','HEAD'],cwd=ROOT,capture_output=True,text=True)
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'status': 'running', 'passed': False,
              'scope': 'Pinned native core binary with original assertions and C++14. Explicit incompatible units remain inventoried unsupported and fail the full gate. Only requested original cases execute; external-service support is not inferred.',
              'source_commit': COMMIT, 'repository_commit': repository.stdout.strip() if repository.returncode == 0 else None,
              'repository_git_error': repository.stderr.strip() if repository.returncode else None,
              'commands': [], 'results': []}
    args.output.parent.mkdir(parents=True, exist_ok=True)

    def checkpoint():
        temporary = args.output.with_suffix('.next.json'); write_report(temporary, report); temporary.replace(args.output)

    def run(command, cwd, log, timeout=300, phase='inventory'):
        return run_command(command, cwd, log, timeout, report, checkpoint, phase)

    try:
        require_build_space(ROOT)
        source = args.source.resolve(strict=True); cache = args.cache.resolve(); cache.mkdir(parents=True, exist_ok=True)
        if source == cache or source in cache.parents: raise ValueError('Cache must be outside pinned source')
        modules = json.loads((ROOT/'migration/modules.json').read_text())
        validate(modules)
        excluded = {}
        for unit in args.exclude_unit:
            path = Path(unit)
            if not unit.startswith('core-tests/src/') or path.suffix != '.cpp' or '..' in path.parts:
                raise ValueError('Invalid excluded original unit: ' + unit)
            body = subprocess.check_output(['git','-C',str(source),'show',COMMIT+':'+unit])
            declarations = re.findall(r'\bTEST(?:_F|_P)?\s*\(\s*(\w+)\s*,\s*(\w+)\s*\)',body.decode())
            excluded[unit] = {'sha256':hashlib.sha256(body).hexdigest(), 'declared_cases':[a+'.'+b for a,b in declarations], 'status':'unsupported', 'reason':'Explicit configured original build incompatibility; assertions unchanged and unexecuted'}
        report['excluded_units'] = excluded
        archive = args.archive.resolve(strict=True) if args.archive else None
        compiler = subprocess.check_output(['c++','--version'],text=True).splitlines()[0]
        ccache = args.ccache.resolve(strict=True) if args.ccache else None
        inputs = {'ccache': {'path':str(ccache), 'sha256':sha(ccache), 'version':subprocess.check_output([str(ccache),'--version'],text=True).splitlines()[0]} if ccache else None,
                  'pin': COMMIT, 'config_sha256': sha(args.config),
                  'hybrid': hybrid_inputs(archive) if archive else None, 'archive_sha256': sha(archive) if archive else None,
                  'compiler': compiler, 'cmake': subprocess.check_output(['cmake','--version'],text=True).splitlines()[0],
                  'dependencies': subprocess.check_output(['pkg-config','--modversion','openssl','libnghttp2','libpq','libcurl','gtest'],text=True).splitlines(),
                  'openssl_dir': str(args.openssl_dir.resolve()) if args.openssl_dir else None,
                  'platform': platform.platform(), 'php_config': subprocess.check_output([str(Path(args.php_dir)/'bin/php-config'),'--version'],text=True).strip(),
                  'excluded_units': excluded, 'runner_sha256': sha(__file__), 'patch_helper_sha256': sha(ROOT/'tools/build_hybrid.py'),
                  'darwin_stat_adapter': args.darwin_stat_adapter,
                  'environment': {k:os.environ.get(k,'') for k in ['CC','CXX','CFLAGS','CXXFLAGS','CPPFLAGS','LDFLAGS','PKG_CONFIG_PATH','CMAKE_PREFIX_PATH','SWOOLE_RS_TOOLCHAIN_ID','LD_LIBRARY_PATH','LD_PRELOAD','DYLD_LIBRARY_PATH','DYLD_FALLBACK_LIBRARY_PATH','DYLD_INSERT_LIBRARIES']}}
        identity = hashlib.sha256(json.dumps(inputs,sort_keys=True).encode()).hexdigest()
        work = cache / identity; tree = work / 'src'; build = work / 'build'; binary = build / 'bin/core-tests'; receipt = work/'build-receipt.json'
        library = tree / ('lib/libswoole.dylib' if platform.system() == 'Darwin' else 'lib/libswoole.so')
        attempt = work / 'attempts' / uuid.uuid4().hex
        attempt.mkdir(parents=True,exist_ok=True)
        report.update(inputs=inputs, identity=identity, cache=str(work), artifacts=str(attempt))
        lock_started = time.monotonic()
        with exclusive(cache / (identity + '.lock')):
            report.setdefault('timings_seconds', {})['cache_lock_wait'] = round(time.monotonic() - lock_started, 4)
            with timed_phase(report, 'cache_validation'):
                valid = valid_build_receipt(receipt, binary, library)
                report.update(cache_hit=valid, cache_reason=('verified executable and native-library identities' if valid else
                              'cached executable/library mismatch or unreadable receipt' if receipt.exists() else
                              'no build receipt for this exact input identity'))
            checkpoint()
            if receipt.exists() and not valid:
                raise ValueError('Cached executable/library identity mismatch; preserve artifacts and investigate before rebuilding')
            if not valid:
                require_build_space(cache)
                with timed_phase(report, 'prepare'):
                    tree.mkdir(parents=True,exist_ok=True)
                    report['original_assertion_files'] = stage(source,tree)
                    shutil.copyfile(args.config,tree/'config.h')
                    if sha(tree/'config.h') != inputs['config_sha256']: raise ValueError('Config changed during capture')
                    if excluded:
                        cmake = tree/'CMakeLists.txt'
                        text = cmake.read_text()
                        marker = 'add_executable(core-tests ${core_test_files})'
                        if text.count(marker) != 1: raise ValueError('Original core target changed')
                        adapter = 'list(REMOVE_ITEM core_test_files ' + ' '.join(json.dumps(str(tree/unit)) for unit in excluded) + ')\n'
                        cmake.write_text(text.replace(marker,adapter+marker))
                    adapter = stat_adapter(tree, args.darwin_stat_adapter, platform.system())
                    if adapter:
                        with (tree/'CMakeLists.txt').open('a') as f:
                            f.write(adapter)
                    if archive:
                        hybrid_cmake = overlay_hybrid_tree(archive, tree)
                        if hybrid_inputs(archive) != inputs['hybrid']: raise ValueError('Hybrid tree changed during capture')
                        frozen_archive = work/'libswoole_rs_ffi.a'
                        shutil.copyfile(archive,frozen_archive)
                        if sha(frozen_archive) != inputs['archive_sha256']: raise ValueError('Archive changed during capture')
                        # Build-only adapter: original CMake/tests stay intact; add exact Rust dependency.
                        with (tree/'CMakeLists.txt').open('a') as f:
                            f.write('\n'+hybrid_cmake+'target_link_libraries(lib-swoole '+json.dumps(str(frozen_archive))+' m dl pthread)\n')
                    report['cmake_adapter_sha256'] = sha(tree/'CMakeLists.txt')
                    launchers = []
                    debug_map = '-fdebug-prefix-map='+str(work)+'=/swoole-native-originals' if ccache else ''
                    if ccache:
                        launcher = ';'.join(['env', 'CCACHE_BASEDIR=' + str(work), str(ccache)])
                        launchers = ['-DCMAKE_C_COMPILER_LAUNCHER='+launcher,'-DCMAKE_CXX_COMPILER_LAUNCHER='+launcher]
                        report['compiler_cache'] = {'launcher':launcher}
                    openssl = ['-Dopenssl_dir='+str(args.openssl_dir.resolve())] if args.openssl_dir else []
                run(['cmake','-S',tree,'-B',build,'-DCMAKE_BUILD_TYPE=Release','-Dphp_dir='+args.php_dir,
                     '-DCMAKE_CXX_FLAGS_RELEASE=-O2 -UNDEBUG','-DCMAKE_C_FLAGS_RELEASE=-O2 -UNDEBUG',
                     '-DCMAKE_CXX_FLAGS='+debug_map,'-DCMAKE_C_FLAGS='+debug_map,*openssl,*launchers],tree,attempt/'configure.log',phase='configure')
                run(['cmake','--build',build,'--target','core-tests','-j'+str(args.jobs),'--','-k'],tree,attempt/'build.log',timeout=900,phase='build')
                receipt.write_text(json.dumps({'inputs':inputs,'binary_sha256':sha(binary),'native_library':library_identity(library),'assertions':report['original_assertion_files'],'cmake_adapter_sha256':report['cmake_adapter_sha256']},indent=2)+'\n')
            built = json.loads(receipt.read_text()); report.update(binary_sha256=sha(binary),build_receipt_sha256=sha(receipt),original_assertion_files=built['assertions'],cache_hit=valid)
            executable = fixture_binary(binary,tree)
            report['execution_binary'] = str(executable)
            report['execution_binary_sha256'] = sha(executable)
            expected_library = built['native_library']
            report['native_library'] = expected_library
            with timed_phase(report, 'loader_probe'):
                report['loader_probe'] = verify_loader(executable, tree, expected_library, attempt/'loader.log')
            checkpoint()
            listing = run([executable,'--gtest_list_tests'],tree,attempt/'inventory.log')
            inventory = inventory_from_listing(listing)
            if not inventory: raise ValueError('No runtime original test inventory')
            selected_cases = [name for name in inventory if any(fnmatch.fnmatchcase(name,pattern) for pattern in args.case)]
            missing = [pattern for pattern in args.case if not any(fnmatch.fnmatchcase(name,pattern) for name in inventory)]
            report.update(inventory=inventory,unselected=[n for n in inventory if n not in selected_cases],unexecuted=inventory.copy(),unknown_selections=missing)
            if missing: raise ValueError('Unknown requested test patterns: ' + str(missing))
            for index,name in enumerate(selected_cases):
                xml = attempt/(str(index)+'-result.xml'); log=attempt/(str(index)+'-case.log')
                if xml.exists(): xml.unlink()
                row = {'name':name,'status':'failed','log':str(log), 'timing_basis':'isolated_wall_clock'}
                case_started = time.monotonic()
                try:
                    row['native_library_before'] = library_identity(library)
                    if row['native_library_before'] != expected_library:
                        raise ValueError('Native library changed before original case')
                    result=run_isolated([str(executable),'--gtest_filter='+name,'--gtest_output=xml:'+str(xml)],cwd=tree,timeout=args.timeout)
                    log.write_text(result.stdout+result.stderr)
                    row.update(exit=result.returncode,**test_outcome(xml,name,result.returncode))
                except subprocess.TimeoutExpired as error:
                    log.write_text(str(error.stdout)+str(error.stderr))
                    row['error']='Original test timed out: '+str(error)
                finally:
                    row['seconds'] = round(time.monotonic() - case_started, 4)
                    timings = report.setdefault('timings_seconds', {})
                    timings['cases'] = round(timings.get('cases', 0) + row['seconds'], 4)
                row['native_library_after'] = library_identity(library)
                if row['native_library_after'] != expected_library:
                    row.update(status='failed', error='Native library changed during original case')
                row['log_sha256']=sha(log)
                report['results'].append(row)
                report['unexecuted'] = [n for n in inventory if n not in {r['name'] for r in report['results']}]
                checkpoint()
            report['status']='complete'; report['selected_cases_passed']=bool(report['results']) and all(x['status']=='passed' for x in report['results'])
            report['passed']=report['selected_cases_passed'] and not excluded
            report['whole_build_inventory_supported']=not excluded
    except BaseException as error:
        report['status']='interrupted' if isinstance(error,KeyboardInterrupt) else 'failed';report['error']=str(error)
    finally: checkpoint()
    print(json.dumps({k:report.get(k) for k in ['identity','status','passed','error']}))
    return int(report['status']!='complete' or bool(args.exclude_unit) or (bool(args.case) and not report['passed']))


if __name__ == '__main__':
    sys.exit(main())
