#!/usr/bin/env python3
"""Exercise real Event descriptor, loop and callback ownership on each extension."""
import argparse
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import parse_engines, run_isolated

CASES = {
    'dispatch_without_reactor': ("var_dump(Swoole\\Event::dispatch()); Swoole\\Event::wait(); Swoole\\Event::exit();", 'bool(false)\n'),
    'defer_order': (r'''
        $out = [];
        Swoole\Event::defer(function() use (&$out) { $out[] = 1;
            Swoole\Event::defer(function() use (&$out) { $out[] = 3; }); });
        Swoole\Event::defer(function() use (&$out) { $out[] = 2; });
        Swoole\Event::wait(); echo json_encode($out), "\n";
    ''', '[1,2,3]\n'),
    'self_replace_remove': (r'''
        $pair = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, 0);
        $out = [];
        Swoole\Event::add($pair[0], function($fd) use (&$out) {
            $out[] = fread($fd, 1);
            Swoole\Event::set($fd, function() { throw new Exception('replaced callback ran'); });
            $out[] = Swoole\Event::del($fd);
        });
        fwrite($pair[1], 'a');
        $out[] = Swoole\Event::dispatch();
        Swoole\Event::wait();
        fclose($pair[0]); fclose($pair[1]);
        echo json_encode($out), "\n";
    ''', '["a",true,true]\n'),
    'cycle_self_replace': (r'''
        $out = [];
        Swoole\Event::cycle(function() use (&$out) {
            $out[] = 1;
            Swoole\Event::cycle(function() use (&$out) {
                $out[] = 2;
                Swoole\Event::cycle(null);
            }, true);
            Swoole\Event::cycle(null);
        });
        Swoole\Event::defer(function() {});
        Swoole\Event::wait();
        echo json_encode($out), "\n";
    ''', '[1,2]\n'),
    # The nested pass must not free the cycle task its callback just cleared.
    'cycle_clear_dispatch': (r'''
        $out = [];
        Swoole\Event::cycle(function() use (&$out) {
            Swoole\Event::cycle(null);
            $out[] = 'cleared';
            Swoole\Event::dispatch();
            $out[] = 'after';
        });
        Swoole\Event::defer(function() use (&$out) { $out[] = 'deferred'; });
        Swoole\Event::wait();
        echo json_encode($out), "\n";
    ''', '["deferred","cleared","after"]\n'),
    'descriptors': (r'''
        $warnings = [];
        set_error_handler(function($level, $message) use (&$warnings) {
            $warnings[] = preg_replace('/^.*?: /', '', $message); return true;
        });
        foreach ([-1, false, new stdClass()] as $candidate) {
            var_dump(Swoole\Event::add($candidate, function() {}));
        }
        echo json_encode($warnings), "\n";
    ''', None),
    'socket_object_descriptor': (r'''
        $socket = new Swoole\Coroutine\Socket(AF_INET, SOCK_DGRAM, 0);
        var_dump(Swoole\Event::add($socket, function() {}) === $socket->fd);
        var_dump(Swoole\Event::isset($socket));
        var_dump(Swoole\Event::del($socket));
        Swoole\Event::wait(); $socket->close();
    ''', 'bool(true)\nbool(true)\nbool(true)\n'),
    'callback_destructor': (r'''
        class Callback {
            public function __invoke() { echo "called\n"; }
            public function __destruct() { echo "destroyed\n"; }
        }
        $callback = new Callback;
        Swoole\Event::defer($callback); unset($callback);
        Swoole\Event::wait(); echo "done\n";
    ''', 'called\ndestroyed\ndone\n'),
}

SOCKET_CASE = (r"""
    $pair = [];
    if (!socket_create_pair(AF_UNIX, SOCK_STREAM, 0, $pair)) { exit(21); }
    $out = [];
    Swoole\Event::add($pair[0], function($socket) use (&$out) {
        $out[] = socket_read($socket, 1);
        $out[] = Swoole\Event::del($socket);
    });
    socket_write($pair[1], 'x');
    Swoole\Event::wait();
    socket_close($pair[0]); socket_close($pair[1]);
    echo json_encode($out), "\n";
""", '["x",true]\n', 0)


BAILOUTS = {
    'deferred_exit': (r"Swoole\Event::defer(function() { echo 'deferred'; exit(7); }); Swoole\Event::wait();", None, 255),
    'cycle_exit': (r"Swoole\Event::cycle(function() { echo 'cycle'; exit(8); }); Swoole\Event::defer(function() {}); Swoole\Event::wait();", None, 255),
    'warning_exit': (r"set_error_handler(function() { echo 'caught'; exit(9); }); Swoole\Event::add(-1, function() {});", 'caughtcaught', 9),
}



def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--engine', action='append', required=True)
    p.add_argument('--php', default='php')
    p.add_argument('--php-extension', action='append', default=[])
    p.add_argument('--php-sockets', action='store_true', help='Require the enabled PHP sockets integration profile')
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    report = {'passed': False, 'engines': [], 'cases': [],
              'unmeasured': [] if a.php_sockets else ['PHP socket object integration: profile not selected']}
    a.output.parent.mkdir(parents=True, exist_ok=True)
    observations = {}
    for engine in parse_engines(a.engine, a.php, a.php_extension):
        report['engines'].append(engine.metadata())
        cases = {name: (*case, 0) for name, case in CASES.items()} | BAILOUTS
        if a.php_sockets:
            cases['php_socket_object'] = SOCKET_CASE
        for name, (code, expected, status) in cases.items():
            result = run_isolated(engine.command('-r', code), timeout=10)
            row = dict(engine=engine.name, case=name, exit=result.returncode,
                       stdout=result.stdout, stderr=result.stderr)
            report['cases'].append(row)
            a.output.write_text(json.dumps(report, indent=2) + '\n')
            observation = (result.returncode, result.stdout, result.stderr)
            if result.returncode != status or result.stderr or (expected is not None and result.stdout != expected):
                print(json.dumps(row), file=sys.stderr)
                return 1
            if name in observations and observations[name] != observation:
                print(json.dumps(row), file=sys.stderr)
                return 1
            observations[name] = observation
    report['passed'] = True
    a.output.write_text(json.dumps(report, indent=2) + '\n')
    print(f'{len(CASES) + len(BAILOUTS) + int(a.php_sockets)} Event lifecycle cases passed on {len(report["engines"])} engines')
    return 0


if __name__ == '__main__':
    sys.exit(main())
