#!/usr/bin/env python3
"""Check real PHP startup mkdir, native logging and global last_error behavior."""
import argparse
import errno
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import sys
import shutil
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import parse_engines, run_isolated
from tools.ext_identity import attest, unchanged


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--build-manifest', type=Path, required=True)
    args = parser.parse_args()
    report = dict(passed=False, created_utc=datetime.now(timezone.utc).isoformat(), engines=[], probes=[],
                  scope='Actual PHP MINIT sys_temp_dir -> set_task_tmpdir -> recursive mkdir; real native logging/last_error, private paths only. No fixed-path full-core case/performance claim.')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    report['php_command'] = str(Path(shutil.which(args.php) or args.php).resolve())
    code = r'''
$path = $argv[1]; clearstatcache(true, $path); $exists = is_dir($path);
echo "\nM19_STARTUP ", json_encode(['exists' => $exists, 'last_error' => swoole_last_error(),
    'mode' => $exists ? fileperms($path) & 0777 : null], JSON_THROW_ON_ERROR), "\n";
'''
    try:
        engines = parse_engines(args.engine, args.php, args.php_extension)
        report['engines'] = [engine.metadata() for engine in engines]
        if {bool(item['rust_feature_bits'] & 8192) for item in report['engines']} != {False, True}:
            raise ValueError('Need actual native and Rust-corebase engines')
        selected = next(item for item in report['engines'] if item['rust_feature_bits'] & 8192)
        report['source_attestation'] = attest(args.build_manifest, selected, 8192)
        with tempfile.TemporaryDirectory(prefix='swoole-mkdir-php-') as temporary:
            results = {}
            for engine in engines:
                results[engine.name] = []
                for name in ['create', 'repeated', 'file-parent']:
                    root = Path(temporary) / engine.name / name;root.mkdir(parents=True)
                    if name == 'file-parent':
                        (root / 'file').write_text('untouched')
                        target = str(root / 'file/child')
                    else:
                        target = str(root) + ('/a//b/' if name == 'repeated' else '/a/b')
                    # PHP alone must not create this path; bind the side effect to Swoole startup.
                    control = run_isolated([args.php, '-n', '-d', 'sys_temp_dir=' + target, '-r',
                        'echo json_encode([extension_loaded("swoole"),is_dir($argv[1])]);', target], timeout=30)
                    assert control.returncode == 0 and not control.stderr and control.stdout == '[false,false]'
                    probe = run_isolated(engine.command('-d', 'sys_temp_dir=' + target, '-r', code, target), timeout=30)
                    entry = dict(engine=engine.name, case=name, returncode=probe.returncode, stdout=probe.stdout, stderr=probe.stderr)
                    report['probes'].append(entry)
                    if probe.returncode or probe.stderr or probe.stdout.count('M19_STARTUP ') != 1:
                        raise RuntimeError('Startup probe failed; diagnostics retained')
                    warnings, payload = probe.stdout.split('M19_STARTUP ')
                    result = json.loads(payload)
                    normalized = [re.sub(r'^\[[^\]]+\]\s*', '', line).replace(str(root), '{root}')
                                  for line in warnings.splitlines() if line.strip()]
                    entry.update(result=result, normalized_messages=normalized)
                    assert result['exists'] == (name != 'file-parent')
                    if name == 'file-parent':
                        assert result['last_error'] == errno.ENOTDIR and len(normalized) == 2
                        assert (root / 'file').read_text() == 'untouched'
                    else:
                        assert not normalized
                    results[engine.name].append((result, normalized))
            assert all(value == results[engines[0].name] for value in results.values())
        unchanged(args.build_manifest, report['source_attestation'])
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    finally:
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(dict(passed=report['passed'], probes=len(report['probes']), error=report.get('error'))))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
