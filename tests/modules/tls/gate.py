#!/usr/bin/env python3
"""Canonical selected-FFI TLS gate: an attested pinned native runtime against one built from the hybrid tree."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import uuid

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import run_isolated

from tools.build_hybrid import hybrid_tree
from tools.build_upstream import COMMIT as PIN


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


class PreconditionError(ValueError):
    pass


def control_from_report(path, config, *, allow_outer=True):
    """Read only successfully built, unselected same-config native control."""
    raw=path.read_bytes()
    report = json.loads(raw)
    identity=dict(path=str(path.resolve()),sha256=hashlib.sha256(raw).hexdigest())
    native=report.get('builds',{}).get('native')
    if native is not None:
        if not allow_outer or not isinstance(native,dict):
            raise PreconditionError('Nested native attestation must contain one direct native-core child')
        child=Path(native['report'])
        if not child.is_absolute() or sha(child)!=native['sha256']:
            raise PreconditionError('Outer native child report path/hash differs from attestation')
        control=control_from_report(child,config,allow_outer=False)
        if control['kind']!='native-core':
            raise PreconditionError('Canonical outer requires a complete original native-core child')
        if control['report_chain'][0]['sha256']!=native['sha256']:
            raise PreconditionError('Native child report changed during attestation')
        control['report_chain'].insert(0,identity)
        control['outer_native']={key:native.get(key) for key in
            ['report','sha256','exit','selected_cases_passed','full_inventory_passed','excluded_units','case_count']}
        control['child_parent']={key:control[key] for key in ['parent_passed','parent_status','parent_error']}
        control.update(parent_passed=report.get('passed'),parent_status=report.get('execution_status',report.get('status')),
                       parent_error=report.get('error'))
        return control
    if report.get('upstream_pin') == PIN and isinstance(report.get('engines'), list):
        rows = [e for e in report['engines'] if e.get('name') == 'upstream']
        if len(rows) != 1:
            raise PreconditionError('Missing/ambiguous upstream native engine')
        kind='coroutine'
        row = rows[0]
        if row.get('configure', {}).get('returncode') != 0 or row.get('build', {}).get('returncode') != 0:
            raise PreconditionError('Native full-runtime configure/build failed; no rebuild attempted')
        command = row['configure']['command']
        tree = Path(command[command.index('-S') + 1]).resolve(strict=True)
        library = (tree/'lib'/('libswoole.dylib' if sys.platform == 'darwin' else 'libswoole.so')).resolve(strict=True)
        expected = row['library_sha256']
        compiled_config = row['compiled_config_sha256']
        openssl = next((Path(v.split('=', 1)[1]) for v in command if v.startswith('-Dopenssl_dir=')), Path('/usr'))
    elif report.get('source_commit') == PIN and report.get('modules') == [] and report.get('native_library'):
        kind='native-core'
        if report.get('status') != 'complete':
            raise PreconditionError('Native core build report incomplete')
        tree = Path(report['cache'])/'src'
        library = Path(report['native_library']['resolved_path']).resolve(strict=True)
        expected = report['native_library']['sha256']
        compiled_config = report['inputs']['config_sha256']
        openssl = Path(report['inputs'].get('openssl_dir') or '/usr')
    else:
        raise PreconditionError('Report lacks attested original full-runtime library; old binary-only cache is insufficient')
    if compiled_config != sha(config) or sha(tree/'config.h') != compiled_config:
        raise PreconditionError('Native library and selected PHP configuration differ')
    if sha(library) != expected:
        raise PreconditionError('Native library differs from attested build')
    return dict(kind=kind,report_chain=[identity],tree=tree, library=library, library_sha256=expected, openssl=openssl.resolve(),
                gtest=report.get('gtest_prefix'), parent_passed=report.get('passed'),
                parent_status=report.get('execution_status', report.get('status')), parent_error=report.get('error'))


def hybrid_library(archive, openssl, jobs, run):
    """lib-swoole built from the hybrid build's fully patched tree, once per hybrid build.

    The patched side's protocol TUs see the patched headers (Socket's layout among them), so
    they need a native library compiled from the same headers, not the pinned control."""
    source, flags = hybrid_tree(archive)
    key = hashlib.sha256(''.join(sha(path) for path in [archive, source/'config.h', source.parent/'state.json']).encode()).hexdigest()
    tree = ROOT/'target/native-core'/('hybrid-'+key[:16])
    library = tree/'lib'/('libswoole.dylib' if sys.platform == 'darwin' else 'libswoole.so')
    if library.is_file():
        return library
    shutil.rmtree(tree, ignore_errors=True)
    listed = run(['git', '-C', source, 'ls-files', '-z', '--cached', '--others', '--exclude-standard'])
    if listed.returncode: raise PreconditionError('Cannot list the hybrid tree')
    for name in [*filter(None, listed.stdout.split('\0')), 'config.h']:
        if (source/name).is_file():
            (tree/name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source/name, tree/name)
    with (tree/'CMakeLists.txt').open('a') as cmake:
        cmake.write('\ntarget_link_libraries(lib-swoole '+json.dumps(str(archive))+' m dl pthread)\n')
    php = Path(shutil.which('php-config') or '/usr/bin/php-config').resolve().parents[1]
    configure = ['cmake', '-S', tree, '-B', tree/'build', '-DCMAKE_BUILD_TYPE=Release', '-Dphp_dir='+str(php),
                 '-DCMAKE_CXX_FLAGS='+' '.join(flags), *(['-Dopenssl_dir='+str(openssl)] if openssl != Path('/usr') else [])]
    for command in [configure, ['cmake', '--build', tree/'build', '--target', 'lib-swoole', '-j'+str(jobs)]]:
        if run(command).returncode: raise RuntimeError('Hybrid native library build failed; see its log')
    return library


def sanitizer_symbols(output):
    return sorted({line.split()[-1] for line in output.splitlines() if line.split() and
                   any(prefix in line.split()[-1] for prefix in ('__asan_', '__ubsan_', '__tsan_', '__msan_'))})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['source', 'config', 'archive', 'output']:
        parser.add_argument('--'+name, type=Path, required=True)
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--gtest-prefix', type=Path)
    parser.add_argument('--native-report', type=Path, help='Attested coroutine upstream/native-core control report, same config')
    parser.add_argument('--allocation-native-report', type=Path, help='Attested same-config ordinary control for C++ allocation when the main native control is instrumented')
    args = parser.parse_args()
    if args.jobs < 1: parser.error('--jobs must be positive')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    attempt = args.output.resolve().parent/(args.output.stem+'-'+uuid.uuid4().hex);attempt.mkdir()
    if args.output.exists(): shutil.copy2(args.output, attempt/'previous-summary.json')
    report = dict(passed=False, status='running', utc=datetime.now(timezone.utc).isoformat(), attempt=str(attempt),
        scope='Actual published FFI archive with module-only protocol TUs, strict original SSL tests, registered callback peers and controlled native C++new failures. No PHP engine, IP Socket, packetloss, cancellation or performance claim.',
        commands=[], jobs_limit=args.jobs, execution='sequential bounded module compilations; the hybrid native library is built once per hybrid build')
    env = os.environ.copy()
    def run(command):
        result = run_isolated(list(map(str, command)), cwd=ROOT, env=env, timeout=900)
        log=attempt/(str(len(report['commands']))+'.log');log.write_text(result.stdout+result.stderr)
        report['commands'].append(dict(argv=list(map(str,command)), returncode=result.returncode, log=str(log)))
        return result
    try:
        for key in ['LD_PRELOAD', 'LD_AUDIT', 'DYLD_INSERT_LIBRARIES']:
            if env.get(key): raise PreconditionError('Injected runtime requires separate matched setup: '+key)
        for key in list(env):
            if key.startswith(('LD_DEBUG', 'LD_TRACE_', 'DYLD_PRINT_')): env.pop(key)
        if run(['git','-C',args.source,'rev-parse','HEAD']).stdout.strip() != PIN:
            raise PreconditionError('Reference checkout must be the pinned upstream commit')
        config=args.config.resolve(strict=True);archive=args.archive.resolve(strict=True)
        report.update(archive_input=str(archive), archive_sha256=sha(archive))
        candidates=[args.native_report] if args.native_report else [args.output.parent/'coroutine-native-originals.json', ROOT/'target/reports/coroutine-native-originals.json']
        failures=[];control=None
        for candidate in candidates:
            try:
                if candidate is None or not candidate.is_file(): raise PreconditionError('Native report absent: '+str(candidate))
                control=control_from_report(candidate,config);shutil.copy2(candidate,attempt/'native-report.json')
                for index,item in enumerate(control['report_chain']):
                    captured=attempt/('native-attestation-'+str(index)+'.json')
                    shutil.copy2(item['path'],captured)
                    if sha(captured)!=item['sha256']: raise PreconditionError('Native attestation changed during capture')
                report.update(native_attestation_chain=control['report_chain'],
                    outer_native=control.get('outer_native'),child_native_parent=control.get('child_parent'),native_report=str(candidate.resolve()),
                    parent_native_passed=control['parent_passed'], parent_native_status=control['parent_status'], parent_native_error=control['parent_error'])
                break
            except (ValueError, OSError, KeyError, IndexError, TypeError) as error: failures.append(str(error))
        if control is None: raise PreconditionError('No coherent attested native control. Supply --native-report; no full build attempted. '+repr(failures))
        headers=attempt/'headers';headers.mkdir();shutil.copytree(control['tree']/'include',headers/'include')
        shutil.copytree(control['tree']/'core-tests/include',headers/'core-tests/include');shutil.copy2(config,headers/'config.h')
        library=attempt/control['library'].name;shutil.copy2(control['library'],library)
        if sha(library)!=control['library_sha256']: raise PreconditionError('Native library changed during capture')
        hybrid=hybrid_library(archive,control['openssl'],args.jobs,run)
        report.update(hybrid_library=str(hybrid),hybrid_library_sha256=sha(hybrid))
        # Query the real shared ABI/feature exports. A convenient standalone kernel is insufficient.
        probe=attempt/'ffi-query.cc';probe.write_text('#include "swoole_rs.h"\n#include <iostream>\nint main(){auto a=swr_v1_abi_version();auto b=swr_v1_feature_bits();std::cout<<a<<" "<<b<<"\\n";return a!=1 || !(b&262144);}\n')
        executable=attempt/'ffi-query'
        command=['c++','-std=c++14','-O2','-I'+str(ROOT/'native/include'),probe,archive,'-pthread','-o',executable]
        if sys.platform=='linux':command+=['-ldl','-lm']
        if run(command).returncode: raise PreconditionError('Actual FFI ABI query failed to link; no substitute archive built')
        queried=run([executable])
        if queried.returncode: raise PreconditionError('Actual FFI ABI1/TLS feature bit missing')
        report['ffi_query']=queried.stdout.strip()
        # Preserve gtest ABI while normalizing Debian multilib layout for the existing gate.
        prefix=(args.gtest_prefix or (Path(control['gtest']) if control['gtest'] else Path('/usr'))).resolve()
        gtest=next((p for p in [prefix/'lib/libgtest.a',*prefix.glob('lib/*/libgtest.a')] if p.is_file()),None)
        if not gtest or not (prefix/'include/gtest/gtest.h').is_file(): raise PreconditionError('Missing compatible GoogleTest prefix')
        staged_gtest=attempt/'gtest';(staged_gtest/'include').mkdir(parents=True);(staged_gtest/'lib').mkdir()
        (staged_gtest/'include/gtest').symlink_to(prefix/'include/gtest',target_is_directory=True);(staged_gtest/'lib/libgtest.a').symlink_to(gtest)
        report['gtest_library_sha256']=sha(gtest)
        report['gates']={}
        allocation_control=control
        for name,extra in [('setup',['--sanitize','--gtest-prefix',staged_gtest]),('peers',['--sanitize','--peers']),('allocation',['--allocation'])]:
            child_library=library
            if name=='allocation':
                report['gates'][name]={'passed':False,'status':'not_run'}
                if args.allocation_native_report:
                    allocation_report_sha=sha(args.allocation_native_report)
                    allocation_control=control_from_report(args.allocation_native_report,config)
                    if allocation_control['openssl']!=control['openssl']:
                        raise PreconditionError('Allocation control OpenSSL prefix differs from setup control')
                    for index,item in enumerate(allocation_control['report_chain']):
                        captured=attempt/('allocation-attestation-'+str(index)+'.json')
                        shutil.copy2(item['path'],captured)
                        if sha(captured)!=item['sha256']:
                            raise PreconditionError('Allocation attestation changed during capture')
                    shutil.copy2(args.allocation_native_report,attempt/'allocation-native-report.json')
                    if sha(attempt/'allocation-native-report.json')!=allocation_report_sha:
                        raise PreconditionError('Allocation native report changed during capture')
                    directory=attempt/'allocation-control';directory.mkdir()
                    child_library=directory/allocation_control['library'].name
                    shutil.copy2(allocation_control['library'],child_library)
                    if sha(child_library)!=allocation_control['library_sha256']:
                        raise PreconditionError('Allocation native library changed during capture')
                symbols=run(['nm','-g',child_library])
                if symbols.returncode: raise PreconditionError('Cannot inspect ordinary allocation control symbols')
                instrumented=sanitizer_symbols(symbols.stdout)
                report['allocation_control']=dict(library=str(child_library),
                    sanitizer_symbols=instrumented,
                    attestation_chain=allocation_control['report_chain'],outer_native=allocation_control.get('outer_native'),
                    child_native_parent=allocation_control.get('child_parent'))
                if instrumented:
                    raise PreconditionError('Ordinary C++ allocation requires an uninstrumented native control; supply --allocation-native-report with an attested same-config control. Sanitized allocation coverage cannot replace the ordinary interposition profile.')
            child=attempt/(name+'.json')
            command=[sys.executable,ROOT/'tests/modules/tls/differential.py','--reference',args.source,'--source',headers,'--library',child_library,'--hybrid-library',hybrid,'--openssl',control['openssl'],'--archive',archive,'--output',child,*extra]
            result=run(command);row=json.loads(child.read_text()) if child.is_file() else {'passed':False,'error':'Child receipt absent'}
            report['gates'][name]={'passed':result.returncode==0 and row.get('passed') is True,'report':str(child)}
            if not report['gates'][name]['passed']: raise RuntimeError('TLS '+name+' failed; first child receipt/logs retained')
        if sha(allocation_control['library'])!=allocation_control['library_sha256']:
            raise RuntimeError('Allocation native control changed during module gate')
        if sha(archive)!=report['archive_sha256'] or sha(control['library'])!=control['library_sha256'] or sha(gtest)!=report['gtest_library_sha256']:
            raise RuntimeError('Input artifact changed during module gate')
        report.update(passed=True,status='complete')
    except Exception as error:
        report.update(status='precondition_failed' if isinstance(error,PreconditionError) else 'failed',error=str(error))
    args.output.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({k:report.get(k)for k in ['passed','status','error','attempt']}))
    return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
