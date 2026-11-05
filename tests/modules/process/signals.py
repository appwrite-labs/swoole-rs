#!/usr/bin/env python3
"""Deliver real kernel signals through configured descriptor and sigaction paths."""
import argparse
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import parse_engines, run_isolated

CASES = {
    'replace_and_unregister': (r'''
        $out = [];
        Swoole\Process::signal(SIGUSR1, function() { throw new Exception('old handler'); });
        Swoole\Process::signal(SIGUSR1, function($signal) use (&$out) {
            $out[] = $signal === SIGUSR1;
            $out[] = Swoole\Process::signal(SIGUSR1, null);
        });
        $out[] = Swoole\Process::kill(getmypid(), SIGUSR1);
        Swoole\Event::wait(); echo json_encode($out), "\n";
    ''', '[true,true,true]\n'),
    'recreate_loop': (r'''
        $out = [];
        for ($i = 0; $i < 3; ++$i) {
            Swoole\Coroutine\run(function() use (&$out, $i) {
                Swoole\Process::signal(SIGUSR1, function() use (&$out, $i) {
                    $out[] = $i; Swoole\Process::signal(SIGUSR1, null);
                });
                Swoole\Process::kill(getmypid(), SIGUSR1);
                Swoole\Coroutine::sleep(0.01);
            });
        }
        echo json_encode($out), "\n";
    ''', '[0,1,2]\n'),
    'two_handlers': (r'''
        $out = [];
        foreach ([SIGUSR1, SIGUSR2] as $signal) {
            Swoole\Process::signal($signal, function($received) use (&$out) {
                $out[] = $received;
                if (count($out) === 2) {
                    Swoole\Process::signal(SIGUSR1, null);
                    Swoole\Process::signal(SIGUSR2, null);
                }
            });
        }
        Swoole\Timer::after(1, function() { Swoole\Process::kill(getmypid(), SIGUSR1); });
        Swoole\Timer::after(5, function() { Swoole\Process::kill(getmypid(), SIGUSR2); });
        Swoole\Event::wait(); sort($out);
        $expected = [SIGUSR1, SIGUSR2]; sort($expected);
        echo json_encode($out === $expected), "\n";
    ''', 'true\n'),

}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    report = dict(passed=False, platform=sys.platform, engines=[], cases=[])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    for engine in parse_engines(args.engine, args.php, args.php_extension):
        report['engines'].append(engine.metadata())
        for descriptor in (False, True):
            settings = json.dumps(dict(enable_signalfd=descriptor, enable_kqueue=descriptor,
                                       wait_signal=True))
            prefix = "swoole_async_set(json_decode('" + settings + "', true));"
            for name, (code, expected) in CASES.items():
                result = run_isolated(engine.command('-r', prefix + code), timeout=12)
                row = dict(engine=engine.name, descriptor=descriptor, case=name,
                           exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
                report['cases'].append(row)
                args.output.write_text(json.dumps(report, indent=2) + '\n')
                if (result.returncode, result.stdout, result.stderr) != (0, expected, ''):
                    print(json.dumps(row), file=sys.stderr)
                    return 1
    report['passed'] = True
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
