#!/usr/bin/env python3
"""Build/load real ZTS+thread native and selected worlds using a verified Rust export.

This reuses immutable compiled Rust bytes; it does not invoke Cargo, substitute
NTS headers for ZTS, exercise thread concurrency by default, or qualify whole parity. The explicit
--locks-threads profile adds only the defined owned-lock/atomic/barrier consumer.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import uuid
from types import SimpleNamespace


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--checkout', type=Path, required=True, help='Frozen first-party source checkout')
    p.add_argument('--source', type=Path, required=True, help='Read-only pinned upstream Git checkout')
    p.add_argument('--rust-export', type=Path, required=True, help='Completed immutable export.json')
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--jobs', type=int, default=4)
    p.add_argument('--php', default='php')
    p.add_argument('--php-config', default='php-config')
    p.add_argument('--phpize', default='phpize')
    p.add_argument('--locks-threads', action='store_true', help='After real ZTS startup, run the owned-lock/atomic/barrier consumer; no cross-owner unlock originals')
    a = p.parse_args()
    sys.path.insert(0, str(a.checkout.resolve()))
    from tools.build_hybrid import MODULES, apply_patch_verified, headers_for, native_compilation, patches_for
    from tools.build_upstream import COMMIT, build_extension, require_build_space
    from tools.check_migration import validate
    from tools.harness import Engine, write_report
    output = a.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    work = output.parent / ('zts-' + uuid.uuid4().hex)
    work.mkdir()
    report = dict(passed=False, scope=__doc__, work=str(work), engines=[],
                  runtime_scope='Real Linux ZTS/thread configure, native build and startup only; no thread execution, original inventory, lifetime/sanitizer/performance or other-platform proof.')
    try:
        if sys.platform != 'linux' or a.jobs < 1:
            raise ValueError('Requires Linux and positive jobs')
        consumer = a.checkout / 'tests/modules/locks/threads.py'
        if a.locks_threads and not consumer.is_file():
            raise ValueError('Frozen checkout lacks the requested real ZTS consumer')
        report['thread_consumer_status'] = 'not_run'
        require_build_space(work)
        php = subprocess.check_output([a.php, '-n', '-r', 'echo json_encode([PHP_VERSION,PHP_ZTS]);'], text=True)
        if json.loads(php)[1] != 1:
            raise ValueError('Actual PHP executable is not ZTS')
        report['php'] = json.loads(php)
        sockets_builtin = subprocess.check_output([a.php, '-n', '-r', "echo extension_loaded('sockets') ? 'yes' : 'no';"], text=True)
        dependencies = () if sockets_builtin == 'yes' else ('sockets',)
        report['php_extensions'] = list(dependencies)
        report['php_config'] = subprocess.check_output([a.php_config, '--configure-options'], text=True)
        if subprocess.check_output([a.php_config, '--version'], text=True).strip() != report['php'][0]:
            raise ValueError('PHP and SDK versions differ')
        pin = subprocess.check_output(['git', '-C', str(a.source), 'rev-parse', 'HEAD'], text=True).strip()
        if pin != COMMIT:
            raise ValueError('Reference pin differs')
        export = json.loads(a.rust_export.read_text())
        for name, identity in export['files'].items():
            if sha(a.rust_export.parent / name) != identity['sha256']:
                raise ValueError('Producer export changed: ' + name)
        producer_path = a.rust_export.parent / 'build-manifest.json'
        producer = json.loads(producer_path.read_text())
        inputs = producer['inputs']
        archive = a.rust_export.parent / 'libswoole_rs_php.a'
        artifact = producer['rust_php_compilation']['artifact']
        if (producer['build_mode'] != 'immutable' or inputs['upstream_commit'] != COMMIT
                or inputs['fix_halfclose'] or sha(archive) != inputs['rust_php_archive_sha256']
                or artifact['target']['kind'] != ['staticlib'] or artifact['profile']['opt_level'] != '3'
                or artifact['profile']['test'] or artifact['profile']['debug_assertions']):
            raise ValueError('Require original-scope immutable optimized Rust producer')
        expected = {**inputs['rust_sources_sha256'], **inputs['headers_sha256'],
                    **{item['path']: item['sha256'] for item in inputs['patches']}}
        for relative, digest in expected.items():
            path = Path(relative)
            if path.is_absolute() or '..' in path.parts or sha(a.checkout / path) != digest:
                raise ValueError('Frozen producer input differs: ' + relative)
        plan = json.loads((a.checkout / 'migration/modules.json').read_text())
        validate(plan)
        modules = inputs['modules']
        selected = {'M00', *(MODULES[name][0] for name in modules)}
        patches = patches_for([m for m in plan['modules'] if m['id'] in selected])
        if patches != inputs['patches']:
            raise ValueError('Selected overlay plan differs from producer')
        report['producer'] = dict(source_ref=export['source_ref'], archive_sha256=sha(archive),
                                 rust_compilation=producer['rust_php_compilation'],
                                 scope='Reused exact producer Rust archive; no Rust compilation occurred in this SDK.')
        packed = subprocess.check_output(['git', '-C', str(a.source), 'archive', COMMIT])
        args = SimpleNamespace(jobs=a.jobs, php_config=a.php_config, phpize=a.phpize,
                               stdext='--enable-swoole-stdext' in producer['configure'],
                               openssl_dir=None, ccache=shutil.which('ccache'))
        prefix = work / 'rust-prefix'
        (prefix / 'include').mkdir(parents=True)
        (prefix / 'lib').mkdir()
        shutil.copy2(archive, prefix / 'lib/libswoole_rs_php.a')
        for header in headers_for(modules):
            shutil.copy2(header, prefix / 'include' / header.name)
        for name in ['native', 'selected']:
            source = work / name
            source.mkdir()
            with tarfile.open(fileobj=io.BytesIO(packed)) as tar:
                for member in tar:
                    path = Path(member.name)
                    if path.is_absolute() or '..' in path.parts:
                        raise ValueError('Unsafe reference archive path')
                    if member.isfile():
                        dest = source / path
                        dest.parent.mkdir(parents=True, exist_ok=True)
                        dest.write_bytes(tar.extractfile(member).read())
                        dest.chmod(member.mode)
            row = dict(name=name, patch_proofs=[], passed=False)
            report['engines'].append(row)
            write_report(output, report)
            extra = ['--enable-swoole-thread', '--enable-sockets']
            if name == 'selected':
                for patch in patches:
                    row['patch_proofs'].append(apply_patch_verified(source, a.checkout / patch['path'],
                                                                   patch['upstream_paths'], patch['sha256']))
                extra += ['--with-swoole-rs=' + str(prefix), *['--enable-swoole-rs-' + m for m in modules]]
            log = work / (name + '-build.log')
            row['configure'] = build_extension(source, log, args, extra)
            extension = source / 'modules/swoole.so'
            row['native_compilation'] = native_compilation(source, log)
            row['extension'] = str(extension)
            row['extension_sha256'] = sha(extension)
            row['metadata'] = Engine(name, extension, a.php, dependencies).metadata()
            check = subprocess.run(Engine(name, extension, a.php, dependencies).command(
                                    '-r', 'echo json_encode(class_exists("Swoole\\\\Thread\\\\Lock"));'),
                                   text=True, capture_output=True, timeout=30)
            row['thread_class_probe'] = dict(exit=check.returncode, stdout=check.stdout, stderr=check.stderr)
            row['passed'] = (row['metadata']['zts'] == 1 and row['metadata']['rust_feature_bits'] ==
                             (producer['migrated_feature_bits'] if name == 'selected' else 0)
                             and 'php_sockets' in row['metadata']['capabilities']
                             and check.returncode == 0 and check.stdout == 'true' and not check.stderr)
            write_report(output, report)
            if not row['passed']:
                raise ValueError('ZTS startup identity/capability differs')
        if a.locks_threads:
            consumer_output = output.with_name(output.stem + '-locks-threads.json')
            command = [sys.executable, str(consumer), '--php', a.php,
                       *[arg for extension in dependencies for arg in ['--php-extension', extension]],
                       '--output', str(consumer_output)]
            for row in report['engines']:
                command += ['--engine', row['name'] + '=' + row['extension']]
            result = subprocess.run(command, text=True, capture_output=True, timeout=120)
            report['thread_consumer'] = dict(command=command, exit=result.returncode,
                                             stdout=result.stdout, stderr=result.stderr)
            if consumer_output.is_file():
                outcome = json.loads(consumer_output.read_text())
                report['thread_consumer']['outcome'] = outcome
            else:
                outcome = {}
            report['thread_consumer_status'] = 'passed' if result.returncode == 0 and outcome.get('passed') is True else 'failed'
            if report['thread_consumer_status'] != 'passed':
                raise ValueError('Defined real ZTS thread consumer failed; first result retained')
            if any(sha(row['extension']) != row['extension_sha256'] for row in report['engines']):
                raise ValueError('ZTS extension changed during consumer execution')
            report['runtime_scope'] = 'Real Linux ZTS build/startup plus defined owned-lock/atomic/barrier consumer only; complete originals, lifetime/sanitizer/performance and other platforms remain unqualified.'
        if sha(archive) != inputs['rust_php_archive_sha256']:
            raise ValueError('Producer archive changed during native build')
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
    write_report(output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error', 'work']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
