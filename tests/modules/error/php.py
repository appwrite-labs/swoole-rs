#!/usr/bin/env python3
"""Check actual PHP error dispatch and an unchanged original PHPT on both engines."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import re
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from corpus.upstream.run import COMMIT, prepare, record_result, run_cases, save_report
from tools.harness import parse_engines, run_isolated


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    report = dict(created_utc=datetime.now(timezone.utc).isoformat(), upstream_commit=COMMIT,
                  passed=False, execution_status='running', engines=[], results=[], probes=[])
    narrowing_path = Path(__file__).with_name('php-narrowing-cases.json')
    paths = ['tests/swoole_function/swoole_strerror.phpt', 'tests/swoole_function/swoole_error_log.phpt']
    try:
        narrowing_cases = json.loads(narrowing_path.read_text())
        if len(narrowing_cases) != 43 or any(type(value) is not int for value in narrowing_cases):
            raise ValueError('Expected the fixed 43-case PHP integer narrowing corpus')
        cases = []
        for path in paths:
            original = subprocess.check_output(['git', '-C', str(args.source), 'show', f'{COMMIT}:{path}'])
            cases.append(dict(path=path, module='error', sha256=hashlib.sha256(original).hexdigest()))
        code = r'''
$codes = array_merge(range(-128, 10276), [PHP_INT_MIN, PHP_INT_MAX]);
$hash = hash_init('sha256'); $count = 0;
foreach ($codes as $code) {
    foreach ([0, SWOOLE_STRERROR_SWOOLE, SWOOLE_STRERROR_SYSTEM] as $kind) {
        $message = swoole_strerror($code, $kind);
        hash_update($hash, json_encode([$code, $kind, $message], JSON_THROW_ON_ERROR)."\n");
        $count++;
    }
}
if (PHP_INT_SIZE !== 8) { throw new RuntimeException('The narrowing corpus requires 64-bit PHP integers'); }
$narrowingCodes = json_decode(__NARROWING_JSON__, true, 512, JSON_THROW_ON_ERROR);
if (!is_array($narrowingCodes) || count($narrowingCodes) !== 43) {
    throw new RuntimeException('Invalid PHP integer narrowing corpus');
}
foreach ($narrowingCodes as $code) {
    if (!is_int($code)) { throw new RuntimeException('Narrowing corpus value did not remain a PHP integer'); }
}
$selectors = [
    ['label' => 'omitted'],
    ['label' => 'literal-zero', 'value' => 0],
    ['label' => 'system-constant', 'value' => SWOOLE_STRERROR_SYSTEM],
    ['label' => 'swoole-constant', 'value' => SWOOLE_STRERROR_SWOOLE],
];
$rows = [];
foreach ($narrowingCodes as $code) {
    foreach ($selectors as $selector) {
        $message = $selector['label'] === 'omitted'
            ? swoole_strerror($code)
            : swoole_strerror($code, $selector['value']);
        $rows[] = ['argument' => $code, 'selector' => $selector['label'],
                   'message' => $message, 'length' => strlen($message)];
    }
}
$boundary = json_encode($rows, JSON_THROW_ON_ERROR | JSON_UNESCAPED_SLASHES);
echo json_encode([
    'calls' => $count,
    'sha256' => hash_final($hash),
    'php_int_size' => PHP_INT_SIZE,
    'narrowing' => ['calls' => count($rows), 'sha256' => hash('sha256', $boundary), 'rows' => $rows],
], JSON_THROW_ON_ERROR | JSON_UNESCAPED_SLASHES);
'''
        code = code.replace('__NARROWING_JSON__', json.dumps(json.dumps(narrowing_cases, separators=(',', ':'))))
        engines = parse_engines(args.engine, args.php, args.php_extension)
        report['engines'] = [engine.metadata() for engine in engines]
        if {bool(metadata['rust_feature_bits'] & 32) for metadata in report['engines']} != {False, True}:
            raise ValueError('Need both native-error and Rust-error engines, verified by feature bit32')
        for engine in engines:
            probe = run_isolated(engine.command('-r', code), timeout=30)
            if probe.returncode or probe.stderr:
                raise RuntimeError(probe.stdout + probe.stderr)
            report['probes'].append(dict(engine=engine.name, **json.loads(probe.stdout)))
            with tempfile.TemporaryDirectory(prefix='swr-error-state-') as temporary:
                log = Path(temporary) / 'error.log'
                state = run_isolated(engine.command(Path(__file__).with_name('state.php'), log), timeout=30)
                report.setdefault('state_probes', []).append(dict(engine=engine.name,
                    returncode=state.returncode, stdout=state.stdout, stderr=state.stderr))
                if state.returncode or state.stderr:
                    raise RuntimeError('PHP state/fork probe failed: ' + state.stdout + state.stderr)
                content = log.read_text()
                report['state_probes'][-1]['log'] = content
                tokens = re.findall(r'#M11:([^#]+)#', content)
                expected = ['empty-parent-visible', 'child-shutdown-visible',
                            'parent-unmodified-visible', 'signed-miss-visible', 'parent-shutdown-visible']
                report['state_probes'][-1].update(tokens=tokens, summary=json.loads(state.stdout))
                if tokens != expected:
                    raise ValueError(f'Incorrect state/fork/shutdown filtering: {tokens}')
            with tempfile.TemporaryDirectory(prefix='swr-errors-php-') as temporary:
                sandbox = Path(temporary)
                prepare(args.source, sandbox, cases)
                (sandbox / 'home').mkdir()
                (sandbox / 'tmp').mkdir()
                artifacts = args.output.resolve().parent / (args.output.stem + '-artifacts') / engine.name
                results, _ = run_cases(engine, cases, sandbox, artifacts, 30, 1)
                for case in cases:
                    record_result(report, engine, case, results[case['path']])
                save_report(args.output, report)
        if len(report['probes']) < 2 or len({p['sha256'] for p in report['probes']}) != 1:
            raise ValueError('Need matching exhaustive PHP results from at least two engines')
        if any(p['php_int_size'] != 8 or p['narrowing']['calls'] != len(narrowing_cases) * 4
               for p in report['probes']):
            raise ValueError('PHP integer narrowing corpus did not execute completely on 64-bit PHP')
        if len({p['narrowing']['sha256'] for p in report['probes']}) != 1:
            raise ValueError('Native and Rust PHP public narrowing results differ')
        report.update(execution_status='complete', passed=all(r['status'] == 'passed' for r in report['results']))
    except Exception as error:
        report.update(execution_status='failed', error=f'{type(error).__name__}: {error}')
    finally:
        save_report(args.output, report)
    print(json.dumps({k: report[k] for k in ('passed', 'execution_status', 'probes')}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
