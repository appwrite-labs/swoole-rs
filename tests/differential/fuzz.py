#!/usr/bin/env python3
"""PHP-API differential fuzzing: short programs of random calls, run on every engine.

Program N of seed S is a pure function of (S, N), so a divergence reproduces from its
two numbers. Each program prints every call's return value, exception or warning; the
first engine is the control. A program whose output, once timestamps and pids are
masked, or whose exit status differs is rerun alone; it is a divergence when it differs
again and the control printed the same both times, and flaky when the control did not.
Divergences are minimised (calls dropped while the divergence stays) and written to
--keep as PHP files. A program that runs out of time on the control (10 s) counts as a
timeout. A signal ending the control is counted but never named in the report or the
log; --private writes those programs to a local directory for a person to triage
privately. --cases also runs each *.php in a directory: kept regressions and scripts
aimed at code the coverage report marks unreached. A case's expected output is the
control's, which must print it twice running, so a case fails the run unless it is
the same. The exit status is 1 when any program diverged or any case failed.

  fuzz.py --engine upstream=EXT --engine cargo=EXT --seed 1 --programs 10000 --output R.json
"""
import argparse
import contextlib
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import random
import re
import subprocess
import sys
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools.harness import parse_engines, run_isolated, write_report

PRELUDE = r'''<?php
set_error_handler(static function (int $severity, string $message): bool {
    echo "  warning($severity): $message\n";
    return true;
});
function r(mixed $value): string {
    if (is_object($value)) return 'object(' . get_class($value) . ')';
    if (is_resource($value)) return 'resource(' . get_resource_type($value) . ')';
    if (!is_array($value)) return var_export($value, true);
    $items = [];
    foreach ($value as $key => $item) $items[] = var_export($key, true) . '=>' . r($item);
    return '[' . implode(',', $items) . ']';
}
function s(string $label, callable $call): mixed {
    try {
        $value = $call();
        echo "$label = ", r($value), "\n";
        return $value;
    } catch (Throwable $error) {
        echo "$label ! ", get_class($error), ': ', $error->getMessage(), "\n";
        return null;
    }
}
'''
# Timestamps, pids and the temporary program's path are the only legitimate differences.
MASKS = [(re.compile(r'\[\d{4}-\d\d-\d\d \d\d:\d\d:\d\d[^\]]*\]'), '[time]'),
         (re.compile(r'\b(pid|process) ?[=#:]? ?\d+', re.I), r'\1 N'),
         (re.compile(r'/[^\s\'"]*/fuzz-\d+\.php'), 'PROGRAM')]
TIMEOUT = 10
VERDICTS = ('same', 'timeout', 'control-crash', 'divergent', 'flaky')


class Gen:
    """Edge-case argument pools and one block per API family."""

    def __init__(self, rng):
        self.rng = rng

    def pick(self, *pool):
        return self.rng.choice(pool)

    def scalar(self):
        return self.pick('0', '1', '-1', '2', '7', '64', '1024', 'PHP_INT_MAX', 'PHP_INT_MIN', '0.5', '-0.0', '1e100',
                         'NAN', 'INF', "''", "'0'", "'1'", "'12abc'", "'x'", 'str_repeat("a", 70)', 'str_repeat("b", 4096)',
                         'null', 'true', 'false', '[]', '[1, 2]', "['k' => 'v']", 'new stdClass')

    def number(self):
        return self.pick('0', '1', '-1', '3', '8', '255', '65536', 'PHP_INT_MAX', 'PHP_INT_MIN', '0.001', '1.5', '-2.5',
                         "'5'", "'abc'", 'null', 'true', '[]')

    def timeout(self):
        # Channel, Atomic and Lock wait for ever on a timeout <= 0.
        return self.pick('0.001', '0.002', '0.01', "'0.001'")

    def sleep(self):
        return self.pick('0.001', '0', '-0.5', "'0.001'", "'x'", 'null')

    def table(self, out):
        # A size near PHP_INT_MAX (or -1, as a size_t) makes create() allocate that much shared memory.
        size = self.pick('1', '2', '7', '64', '1024', '0', "'8'")
        out.append(f"$t = s('table.new', fn() => new Swoole\\Table({size}, {self.pick('0.2', '1', '0', '-1', '100')}));")
        names = ["'i'", "'f'", "'s'", "''", 'str_repeat("c", 80)', "'n'"]
        for name in self.rng.sample(names, self.rng.randint(1, 4)):
            column = self.pick('Swoole\\Table::TYPE_INT', 'Swoole\\Table::TYPE_FLOAT', 'Swoole\\Table::TYPE_STRING', '0', '99', "'1'")
            out.append(f"s('table.column', fn() => $t?->column({name}, {column}, "
                       f"{self.pick('0', '1', '4', '8', '16', '-1', '1000', "'8'")}));")
        if self.rng.random() < .9:
            out.append("s('table.create', fn() => $t?->create());")
        keys = ["'k'", "'k2'", "''", 'str_repeat("k", 63)', 'str_repeat("k", 64)', 'str_repeat("k", 300)', '0', '1.5', "'\\0'"]
        columns = ["'i'", "'f'", "'s'", "'n'", "'missing'", "''"]
        for _ in range(self.rng.randint(2, 12)):
            key, column = self.pick(*keys), self.pick(*columns)
            op = self.pick('set', 'set', 'get', 'get', 'incr', 'decr', 'del', 'exists', 'count', 'getSize', 'column-late', 'iterate', 'stats')
            if op == 'set':
                row = ', '.join(f'{self.pick(*columns)} => {self.scalar()}' for _ in range(self.rng.randint(0, 3)))
                out.append(f"s('table.set', fn() => $t?->set({key}, [{row}]));")
            elif op == 'get':
                out.append(f"s('table.get', fn() => $t?->get({key}{', ' + column if self.rng.random() < .5 else ''}));")
            elif op in ('incr', 'decr'):
                out.append(f"s('table.{op}', fn() => $t?->{op}({key}, {column}, {self.number()}));")
            elif op in ('del', 'exists'):
                out.append(f"s('table.{op}', fn() => $t?->{op}({key}));")
            elif op in ('count', 'getSize'):
                out.append(f"s('table.{op}', fn() => $t?->{op}());")
            elif op == 'column-late':
                out.append("s('table.column', fn() => $t?->column('late', Swoole\\Table::TYPE_INT));")
            elif op == 'stats':
                out.append("s('table.stats', fn() => is_array($x = $t?->stats()) ? array_keys($x) : $x);")
            else:
                out.append("s('table.iterate', fn() => $t ? iterator_to_array($t) : null);")

    def channel(self, out):
        out.append(f"$c = s('chan.new', fn() => new Swoole\\Coroutine\\Channel({self.pick('1', '2', '3', '0', '-1', "'2'", 'PHP_INT_MAX')}));")
        for _ in range(self.rng.randint(2, 12)):
            op = self.pick('push', 'push', 'pop', 'pop', 'close', 'length', 'isEmpty', 'isFull', 'stats', 'errCode', 'go-pop', 'go-push')
            if op == 'push':
                out.append(f"s('chan.push', fn() => $c?->push({self.scalar()}, {self.timeout()}));")
            elif op == 'pop':
                out.append(f"s('chan.pop', fn() => $c?->pop({self.timeout()}));")
            elif op == 'errCode':
                out.append("s('chan.errCode', fn() => $c?->errCode);")
            elif op == 'go-pop':
                out.append("go(function () use ($c) { s('chan.go.pop', fn() => $c?->pop(0.005)); });")
            elif op == 'go-push':
                out.append(f"go(function () use ($c) {{ s('chan.go.push', fn() => $c?->push({self.scalar()}, 0.005)); }});")
            else:
                out.append(f"s('chan.{op}', fn() => $c?->{op}());")

    def server(self, out):
        cls = self.pick('Swoole\\Server', 'Swoole\\Http\\Server', 'Swoole\\WebSocket\\Server')
        out.append(f"$srv = s('server.new', fn() => new {cls}('127.0.0.1', 0, SWOOLE_BASE));")
        options = ['worker_num', 'reactor_num', 'max_conn', 'max_connection', 'backlog', 'dispatch_mode', 'package_max_length',
                   'buffer_output_size', 'socket_buffer_size', 'open_eof_check', 'open_eof_split', 'package_eof',
                   'open_length_check', 'package_length_type', 'package_length_offset', 'package_body_offset',
                   'heartbeat_check_interval', 'heartbeat_idle_time', 'log_level', 'max_request', 'task_worker_num',
                   'open_http_protocol', 'open_websocket_protocol', 'http_compression', 'http_compression_level',
                   'upload_max_filesize', 'enable_coroutine', 'max_coroutine', 'open_tcp_nodelay', 'tcp_fastopen',
                   'buffer_high_watermark', 'buffer_low_watermark', 'input_buffer_size', 'output_buffer_size',
                   'websocket_compression', 'http_parse_post', 'http_parse_cookie', 'document_root', 'enable_static_handler',
                   'static_handler_locations', 'unknown_option', 'hook_flags', 'max_wait_time', 'reload_async']
        for _ in range(self.rng.randint(1, 3)):
            settings = ', '.join(f"'{self.pick(*options)}' => {self.pick(self.scalar(), self.number(), "'N'", "'\\r\\n'")}"
                                 for _ in range(self.rng.randint(1, 5)))
            out.append(f"s('server.set', fn() => $srv?->set([{settings}]));")
        out.append("s('server.setting', fn() => $srv?->setting);")

    def system(self, out):
        path = "__FILE__ . '.data'"
        # A coroutine wait or waitPid installs a SIGCHLD reaper, which races exec's own
        # waitpid for its child, so upstream's exec after one can return either. The
        # exec is still picked, so the rest of the program is unchanged.
        waited = False
        for _ in range(self.rng.randint(2, 8)):
            op = self.pick('sleep', 'usleep', 'write', 'read', 'read-missing', 'wait', 'waitPid', 'gethostbyname', 'exec')
            waited = waited or op in ('wait', 'waitPid')
            if op == 'sleep':
                out.append(f"s('sys.sleep', fn() => Swoole\\Coroutine\\System::sleep({self.sleep()}));")
            elif op == 'usleep':
                out.append(f"s('co.sleep', fn() => Swoole\\Coroutine::sleep({self.sleep()}));")
            elif op == 'write':
                out.append(f"s('sys.writeFile', fn() => Swoole\\Coroutine\\System::writeFile({path}, {self.scalar()}, "
                           f"{self.pick('0', 'FILE_APPEND', '-1', '99')}));")
            elif op == 'read':
                out.append(f"s('sys.readFile', fn() => Swoole\\Coroutine\\System::readFile({path}));")
            elif op == 'read-missing':
                out.append("s('sys.readFile', fn() => Swoole\\Coroutine\\System::readFile('/nonexistent/swr-fuzz'));")
            elif op == 'wait':
                out.append(f"s('sys.wait', fn() => Swoole\\Coroutine\\System::wait({self.timeout()}));")
            elif op == 'waitPid':
                out.append(f"s('sys.waitPid', fn() => Swoole\\Coroutine\\System::waitPid({self.pick('-1', '0', '1', '999999')}, "
                           f"{self.timeout()}));")
            elif op == 'gethostbyname':
                out.append(f"s('sys.gethostbyname', fn() => Swoole\\Coroutine\\System::gethostbyname({self.pick("'localhost'", "'127.0.0.1'", "''", "'::1'")}, "
                           f"{self.pick('AF_INET', 'AF_INET6', '0', '-1')}, 1));")
            else:
                command = self.pick("'echo hi'", "'exit 3'", "''", "'printf \"a\\\\0b\"'")
                if not waited:
                    out.append(f"s('sys.exec', fn() => Swoole\\Coroutine\\System::exec({command}));")
        out.append(f"@unlink({path});")

    def http(self, out):
        if self.rng.random() < .5:
            out.append(f"$req = s('request.create', fn() => Swoole\\Http\\Request::create({self.pick('[]', "['parse_cookie' => false]", "['upload_tmp_dir' => '/nonexistent']", "['max_header_size' => 1]")}));")
            lines = ['GET / HTTP/1.1', 'POST /p?a=1&b[]=2 HTTP/1.1', 'GET http://h/x HTTP/1.0', 'BREW / HTTP/1.1', '']
            headers = ['Host: x', 'Cookie: a=1; b=2', 'Content-Length: 3', 'Content-Length: x', 'Transfer-Encoding: chunked',
                       'Content-Type: application/x-www-form-urlencoded', 'X: \\x00', '']
            text = '\\r\\n'.join([self.pick(*lines), *self.rng.sample(headers, self.rng.randint(0, 3)), '',
                                  self.pick('', 'a=b', 'abc', '3\\r\\nabc\\r\\n0\\r\\n\\r\\n')])
            out.append(f"s('request.parse', fn() => $req?->parse(\"{text}\"));")
            for getter in self.rng.sample(['getMethod', 'getContent', 'getData', 'isCompleted', 'header', 'server', 'cookie', 'get', 'post'],
                                          self.rng.randint(1, 4)):
                call = getter + '()' if getter in ('getMethod', 'getContent', 'getData', 'isCompleted') else getter
                if getter == 'server':  # less the parse time
                    call = "server ? array_diff_key($req->server, ['request_time' => 0, 'request_time_float' => 0]) : null"
                out.append(f"s('request.{getter}', fn() => $req?->{call});")
        else:
            out.append("$res = s('response.new', fn() => new Swoole\\Http\\Response());")
            for _ in range(self.rng.randint(1, 6)):
                op = self.pick('status', 'header', 'cookie', 'rawcookie', 'trailer', 'redirect', 'end', 'write', 'isWritable', 'detach')
                if op == 'status':
                    out.append(f"s('response.status', fn() => $res?->status({self.number()}{', ' + self.scalar() if self.rng.random() < .3 else ''}));")
                elif op == 'header':
                    out.append(f"s('response.header', fn() => $res?->header({self.pick("'X-A'", "''", "'Bad Name'", "'X\\r\\nY'")}, {self.scalar()}));")
                elif op in ('cookie', 'rawcookie'):
                    out.append(f"s('response.{op}', fn() => $res?->{op}({self.pick("'a'", "''", "'a=b'", "'a;b'")}, {self.scalar()}, {self.number()}));")
                elif op == 'trailer':
                    out.append(f"s('response.trailer', fn() => $res?->trailer('T', {self.scalar()}));")
                elif op == 'redirect':
                    out.append(f"s('response.redirect', fn() => $res?->redirect({self.scalar()}, {self.number()}));")
                elif op in ('end', 'write'):
                    out.append(f"s('response.{op}', fn() => $res?->{op}({self.scalar()}));")
                else:
                    out.append(f"s('response.{op}', fn() => $res?->{op}());")

    def atomic(self, out):
        cls = self.pick('Swoole\\Atomic', 'Swoole\\Atomic\\Long')
        out.append(f"$a = s('atomic.new', fn() => new {cls}({self.number()}));")
        for _ in range(self.rng.randint(2, 8)):
            op = self.pick('add', 'sub', 'get', 'set', 'cmpset', 'wait', 'wakeup')
            if op in ('add', 'sub', 'set'):
                out.append(f"s('atomic.{op}', fn() => $a?->{op}({self.number()}));")
            elif op == 'cmpset':
                out.append(f"s('atomic.cmpset', fn() => $a?->cmpset({self.number()}, {self.number()}));")
            elif op == 'wait':
                out.append(f"s('atomic.wait', fn() => $a?->wait({self.timeout()}));")
            elif op == 'wakeup':
                out.append(f"s('atomic.wakeup', fn() => $a?->wakeup({self.pick('', '1', '0', '-1', '100')}));")
            else:
                out.append("s('atomic.get', fn() => $a?->get());")

    def lock(self, out):
        kind = self.pick('SWOOLE_MUTEX', 'SWOOLE_RWLOCK', 'SWOOLE_SPINLOCK', '0', '99', '-1', "'1'")
        out.append(f"$l = s('lock.new', fn() => new Swoole\\Lock({kind}));")
        operation = ('LOCK_EX', 'LOCK_SH', 'LOCK_EX | LOCK_NB', 'LOCK_SH | LOCK_NB', 'LOCK_UN', '0', '99', '-1')
        # A lock() without a timeout blocks for ever on a held lock, so only the first call may omit one.
        if self.rng.random() < .5:
            out.append(f"s('lock.lock', fn() => $l?->lock({self.pick(*operation[:2])}));")
        for _ in range(self.rng.randint(1, 6)):
            if self.rng.random() < .3:
                out.append("s('lock.unlock', fn() => $l?->unlock());")
            else:
                out.append(f"s('lock.lock', fn() => $l?->lock({self.pick(*operation)}, {self.timeout()}));")

    def timer(self, out):
        for _ in range(self.rng.randint(1, 5)):
            op = self.pick('after', 'after', 'tick', 'clear', 'exists', 'stats', 'list', 'clearAll')
            if op == 'after':
                out.append(f"$id = s('timer.after', fn() => Swoole\\Timer::after({self.pick('1', '0', '-1', '2', "'1'", '0.5')}, "
                           "static function () { echo \"  fired after\\n\"; }));")
            elif op == 'tick':
                out.append(f"$id = s('timer.tick', fn() => Swoole\\Timer::tick({self.pick('1', '0', '-1', '3')}, "
                           "static function ($id) { static $n = 0; echo \"  fired tick\\n\"; if (++$n >= 2) Swoole\\Timer::clear($id); }));")
            elif op in ('clear', 'exists'):
                out.append(f"s('timer.{op}', fn() => Swoole\\Timer::{op}({self.pick('$id ?? 0', '0', '-1', '999')}));")
            elif op == 'stats':
                out.append("s('timer.stats', fn() => Swoole\\Timer::stats()['num']);")
            elif op == 'list':
                out.append("s('timer.list', fn() => count(iterator_to_array(Swoole\\Timer::list())));")
            else:
                out.append("s('timer.clearAll', fn() => Swoole\\Timer::clearAll());")
        out.append("Swoole\\Event::wait();")


FAMILIES = {'table': False, 'channel': True, 'server': False, 'system': True, 'http': False,
            'atomic': False, 'lock': False, 'timer': False}


def generate(seed, index):
    """A program as lines; (True, line) may be dropped by minimisation, (False, line) frames it."""
    rng = random.Random(f'{seed}:{index}')
    gen = Gen(rng)
    lines = []
    for family in rng.sample(sorted(FAMILIES), rng.randint(1, 3)):
        calls = []
        getattr(gen, family)(calls)
        if FAMILIES[family]:
            lines += [(False, 'Swoole\\Coroutine\\run(function () {'), *[(True, call) for call in calls], (False, '});')]
        else:
            lines += [(True, call) for call in calls]
    return lines


def source(lines):
    return PRELUDE + '\n'.join(line for _, line in lines) + "\necho \"end\\n\";\n"


def outcome(engine, path):
    try:
        result = run_isolated(engine.command(path), timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return {'status': 'timeout', 'output': ''}
    text = result.stdout + result.stderr
    for pattern, mask in MASKS:
        text = pattern.sub(mask, text)
    return {'status': result.returncode, 'output': text}


def compare(engines, path):
    """Each engine's outcome, and whether all agree with the control."""
    found = [outcome(engine, path) for engine in engines]
    return found, all(item == found[0] for item in found[1:])


class Alone:
    """Programs run side by side; a rerun runs alone, so load on the host can't make a
    divergence out of a timing difference."""

    def __init__(self):
        self.condition = threading.Condition()
        self.running = 0
        self.alone = False

    @contextlib.contextmanager
    def shared(self):
        with self.condition:
            self.condition.wait_for(lambda: not self.alone)
            self.running += 1
        try:
            yield
        finally:
            with self.condition:
                self.running -= 1
                self.condition.notify_all()

    @contextlib.contextmanager
    def exclusive(self):
        with self.condition:
            self.condition.wait_for(lambda: not self.alone)
            self.alone = True
            self.condition.wait_for(lambda: self.running == 0)
        try:
            yield
        finally:
            with self.condition:
                self.alone = False
                self.condition.notify_all()


ALONE = Alone()


def check(engines, path, lines, alone=False):
    """same, timeout (the control ran out of time), control-crash (a signal ended the
    control), flaky or divergent, and the outcomes. `alone`: the caller holds ALONE."""
    path.write_text(source(lines))
    with contextlib.nullcontext() if alone else ALONE.shared():
        first, same = compare(engines, path)
    if first[0]['status'] != 'timeout' and first[0]['status'] < 0:
        return 'control-crash', first
    if same:
        return ('timeout' if first[0]['status'] == 'timeout' else 'same'), first
    with contextlib.nullcontext() if alone else ALONE.exclusive():
        second, same = compare(engines, path)
    return ('divergent' if not same and first[0] == second[0] else 'flaky'), second


def minimise(engines, path, lines):
    """Drop calls while the divergence stays; the caller holds ALONE."""
    for index in reversed(range(len(lines))):
        if lines[index][0] and check(engines, path, lines[:index] + lines[index + 1:], alone=True)[0] == 'divergent':
            lines = lines[:index] + lines[index + 1:]
    return lines


def run_program(engines, workdir, keep, private, seed, index):
    path = workdir / f'fuzz-{index}.php'
    lines = generate(seed, index)
    verdict, found = check(engines, path, lines)
    row = {'program': index, 'verdict': verdict}
    if verdict == 'control-crash':
        # Counted, never named: a person triages these privately (AGENTS.md, security).
        if private:
            private.mkdir(parents=True, exist_ok=True)
            (private / f'seed{seed}-{index}.php').write_text(source(lines))
        path.unlink(missing_ok=True)
        return {'verdict': verdict}
    if verdict == 'divergent':
        with ALONE.exclusive():
            lines = minimise(engines, path, lines)
            again, minimised = check(engines, path, lines, alone=True)
        # The report shows a divergence: the minimised program's, unless it didn't recur.
        if again == 'divergent':
            found = minimised
        row['calls'] = sum(removable for removable, _ in lines)
        if keep:
            keep.mkdir(parents=True, exist_ok=True)
            (keep / f'seed{seed}-{index}.php').write_text(source(lines))
    if verdict != 'same':
        row['outcomes'] = {engine.name: item for engine, item in zip(engines, found)}
    path.unlink(missing_ok=True)
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--engine', action='append', default=[], help='NAME=EXT; the first is the control')
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--seed', type=int, default=0)
    parser.add_argument('--programs', type=int, default=200)
    parser.add_argument('--first', type=int, default=0, help='index of the first program')
    parser.add_argument('--jobs', type=int, default=os.cpu_count() or 2)
    parser.add_argument('--keep', type=Path, help='directory for minimised divergent programs')
    parser.add_argument('--private', type=Path, help='local directory for the programs the control crashed on; never publish it')
    parser.add_argument('--cases', type=Path, help='also run every *.php in DIR: regressions and coverage-targeted cases')
    parser.add_argument('--print', type=int, metavar='INDEX', help='print one program and exit')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args.print is not None:
        print(source(generate(args.seed, args.print)), end='')
        return 0
    engines = parse_engines(args.engine, args.php, args.php_extension)
    if len(engines) < 2:
        parser.error('needs a control and at least one more --engine')
    cases = sorted(args.cases.glob('*.php')) if args.cases else []
    with tempfile.TemporaryDirectory(prefix='swr-fuzz-') as workdir, ThreadPoolExecutor(args.jobs) as pool:
        rows = list(pool.map(lambda index: run_program(engines, Path(workdir), args.keep, args.private, args.seed, index),
                             range(args.first, args.first + args.programs)))
        for case in cases:
            # A case is a minimised program's calls, after its own `<?php` line.
            path = Path(workdir) / 'case.php'
            verdict, found = check(engines, path, [(False, case.read_text().removeprefix('<?php\n'))])
            # A case's expected output is upstream's, so upstream must repeat it.
            if verdict == 'same' and outcome(engines[0], path) != found[0]:
                verdict = 'flaky'
            rows.append({'case': case.name, 'verdict': verdict,
                         **({} if verdict == 'same' else {'outcomes': dict(zip([e.name for e in engines], found))})})
    counts = {verdict: sum(row['verdict'] == verdict for row in rows) for verdict in VERDICTS}
    report = {'seed': args.seed, 'first': args.first, 'programs': args.programs, 'cases': len(cases), **counts,
              'engines': [engine.metadata() for engine in engines],
              'findings': [row for row in rows if row['verdict'] not in ('same', 'control-crash')]}
    if args.output:
        write_report(args.output, report)
    for row in report['findings']:
        print(f"{row['verdict']}: {row.get('case') or f'seed {args.seed} program {row['program']}'}", file=sys.stderr)
    print(json.dumps(counts))
    return 1 if counts['divergent'] or any('case' in row and row['verdict'] != 'same' for row in rows) else 0


if __name__ == '__main__':
    sys.exit(main())
