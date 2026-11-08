#!/usr/bin/env python3
"""Hours-long load on a Swoole server and a process pool, asserting flat resources.

Load: HTTP/1 keep-alive and fresh connections, HTTP/2 (curl), WebSocket echo, task
workers, coroutine redis and mysql clients (--redis/--mysql hosts), and task-worker
reload() cycles, while a process pool respawns its workers. Every --interval seconds
it samples each process's RSS and open fds (/proc, so Linux only) and each server
worker's memory_get_usage().

It fails on a crash, a worker error, a failed request, or growth after --warmup: a
least-squares slope over every process alive for the whole measured window, times the
window, above the --max-* thresholds plus, with --control, the largest growth of that
metric in the control engine's report (upstream under the same load), scaled up when
this run completed more load-driver iterations than the control. --leak runs the server with a per-request leak
and inverts the verdict: it passes only when the growth check catches the leak.
"""
import argparse
import http.client
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.harness import parse_engines, server, stop_process_group, write_report
from corpus.run import WebSocket

HERE = Path(__file__).resolve().parent
MIB = 1024 * 1024


def children():
    """{ppid: [pid, ...]} for every process."""
    tree = {}
    for entry in os.listdir('/proc'):
        if entry.isdigit():
            try:
                stat = Path(f'/proc/{entry}/stat').read_text()
            except OSError:
                continue
            tree.setdefault(int(stat.rsplit(')', 1)[1].split()[1]), []).append(int(entry))
    return tree


def descendants(root, tree):
    found, stack = [], [root]
    while stack:
        pid = stack.pop()
        found.append(pid)
        stack.extend(tree.get(pid, []))
    return found


def resources(pid):
    try:
        status = Path(f'/proc/{pid}/status').read_text()
        rss = next(int(line.split()[1]) * 1024 for line in status.splitlines() if line.startswith('VmRSS:'))
        return rss, len(os.listdir(f'/proc/{pid}/fd'))
    except (OSError, StopIteration):
        return None


def growth(points):
    """Least-squares slope of [(t, value)] times the time they span."""
    n = len(points)
    mean_t = sum(t for t, _ in points) / n
    mean_v = sum(v for _, v in points) / n
    spread = sum((t - mean_t) ** 2 for t, _ in points)
    slope = sum((t - mean_t) * (v - mean_v) for t, v in points) / spread
    return slope * (points[-1][0] - points[0][0])


class Load:
    def __init__(self, port, deadline):
        self.port, self.deadline = port, deadline
        self.counts, self.errors, self.lock = {}, [], threading.Lock()

    def run(self, name, body):
        while time.monotonic() < self.deadline:
            try:
                body()
                with self.lock:
                    self.counts[name] = self.counts.get(name, 0) + 1
            except Exception as error:
                with self.lock:
                    self.errors.append(f'{name}: {type(error).__name__}: {error}')
                time.sleep(1)

    def get(self, connection, path, expected=None):
        connection.request('GET', path)
        response = connection.getresponse()
        body = response.read()
        if response.status != 200 or expected is not None and body != expected:
            raise AssertionError(f'GET {path}: {response.status} {body[:200]!r}')
        return body

    def keepalive(self, paths):
        connection = http.client.HTTPConnection('127.0.0.1', self.port, timeout=10)
        try:
            for _ in range(100):
                for path, expected in paths:
                    self.get(connection, path, expected)
        finally:
            connection.close()

    def fresh(self):
        connection = http.client.HTTPConnection('127.0.0.1', self.port, timeout=10)
        try:
            self.get(connection, '/', b'ok')
        finally:
            connection.close()

    def http2(self):
        url = f'http://127.0.0.1:{self.port}/'
        result = subprocess.run(['curl', '-sS', '--http2-prior-knowledge', '--max-time', '10',
                                 '-w', '%{http_version} %{http_code}\n', *[url] * 50],
                                capture_output=True, text=True, timeout=30)
        lines = result.stdout.replace('ok', '').split()
        if result.returncode or lines != ['2', '200'] * 50:
            raise AssertionError(f'curl exited {result.returncode}: {result.stderr[-300:]} {result.stdout[-300:]}')

    def websocket(self):
        client = WebSocket(self.port)
        try:
            for i in range(100):
                message = f'echo {i}'.encode()
                client.send(message)
                if client.receive() != message:
                    raise AssertionError('WebSocket echo mismatch')
        finally:
            client.close()

    def reload(self):
        connection = http.client.HTTPConnection('127.0.0.1', self.port, timeout=10)
        try:
            self.get(connection, '/reload')
        finally:
            connection.close()
        time.sleep(self.reload_interval)


def stats(port):
    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
    try:
        connection.request('GET', '/stats')
        return json.loads(connection.getresponse().read())
    finally:
        connection.close()


def soak(engine, args):
    env = {'SOAK_LEAK': '1' if args.leak else '0', 'SOAK_REDIS': args.redis or '', 'SOAK_MYSQL': args.mysql or ''}
    pool = subprocess.Popen(engine.command(HERE / 'pool.php'), stdout=subprocess.DEVNULL, start_new_session=True)
    samples, failures = [], []
    started = time.monotonic()
    try:
        with server(engine, HERE / 'server.php', env) as (port, proc):
            load = Load(port, started + args.duration)
            load.reload_interval = args.reload_interval
            paths = [('/', b'ok'), ('/task', b'6')]
            paths += [('/redis', None)] if args.redis else []
            paths += [('/mysql', b'2')] if args.mysql else []
            drivers = {'keepalive': lambda: load.keepalive(paths), 'fresh': load.fresh,
                       'http2': load.http2, 'websocket': load.websocket, 'reload': load.reload}
            threads = [threading.Thread(target=load.run, args=item, daemon=True) for item in drivers.items()]
            for thread in threads:
                thread.start()
            pool_children = set()
            while time.monotonic() < load.deadline:
                time.sleep(args.interval)
                now = time.monotonic() - started
                for name, process in [('server', proc), ('pool', pool)]:
                    if process.poll() is not None:
                        raise AssertionError(f'{name} exited {process.returncode} after {now:.0f}s')
                tree = children()
                pool_children.update(tree.get(pool.pid, []))
                sample = {'t': round(now, 1), 'processes': {}}
                for root, name in [(proc.pid, 'server'), (pool.pid, 'pool')]:
                    for pid in descendants(root, tree):
                        if (measured := resources(pid)) is not None:
                            sample['processes'][f'{name}:{pid}'] = dict(zip(['rss', 'fds'], measured))
                current = stats(port)
                sample['zend'] = {f'worker:{row["pid"]}': row['bytes'] for row in current['memory'].values()}
                sample['worker_errors'] = current['worker_errors']
                sample['requests'] = current['stats'].get('request_count')
                samples.append(sample)
                print(json.dumps(sample), flush=True)
            for thread in threads:
                thread.join(timeout=60)
            final = stats(port)
        if final['worker_errors']:
            failures.append(f'{final["worker_errors"]} worker errors')
        if load.errors:
            failures.append(f'{len(load.errors)} failed requests, first: {load.errors[:5]}')
        if not all(load.counts.get(name) for name in drivers):
            failures.append(f'a load driver never completed: {load.counts}')
    except Exception as error:
        failures.append(f'{type(error).__name__}: {error}')
        load = None
    finally:
        stop_process_group(pool)
    measured = [s for s in samples if s['t'] >= args.warmup]
    limits = {'rss': args.max_rss_mib * MIB, 'fds': args.max_fds, 'zend': args.max_zend_mib * MIB}
    if args.control:
        control = json.loads(Path(args.control).read_text())
        scale = max(1, sum(load.counts.values()) / max(1, sum(control['requests'].values()))) if load else 1
        for metric in limits:
            limits[metric] += scale * max([0, *(v for k, v in control['growth'].items() if k.startswith(f'{metric} '))])
    growths = {}
    for metric, limit in limits.items():
        series = {}
        for sample in measured:
            values = sample['zend'] if metric == 'zend' else {k: v[metric] for k, v in sample['processes'].items()}
            for key, value in values.items():
                series.setdefault(key, []).append((sample['t'], value))
        # Only processes alive for the whole window: reloaded and respawned ones restart from zero.
        for key, points in series.items():
            if len(points) == len(measured) >= 3:
                growths[f'{metric} {key}'] = value = growth(points)
                if value > limit:
                    failures.append(f'{metric} {key} grew {value:.0f} after warm-up (limit {limit})')
    if len(measured) < 3:
        failures.append(f'only {len(measured)} samples after warm-up')
    return {'engine': engine.metadata(), 'duration': args.duration, 'warmup': args.warmup,
            'limits': limits, 'growth': growths, 'failures': failures,
            'requests': load.counts if load else {}, 'pool_workers_started': len(pool_children) if load else 0,
            'samples': samples}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--engine', required=True, help='NAME=/path/to/swoole.so')
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--duration', type=float, default=3600, help='seconds')
    parser.add_argument('--warmup', type=float, help='seconds excluded from the growth fit (default 20%%)')
    parser.add_argument('--interval', type=float, default=10, help='seconds between samples')
    parser.add_argument('--reload-interval', type=float, default=30, help='seconds between task-worker reloads')
    parser.add_argument('--redis', help='redis host for the coroutine client')
    parser.add_argument('--mysql', help='mysql host (root/root, database test) for hooked PDO')
    parser.add_argument('--max-rss-mib', type=float, default=8, help='per process')
    parser.add_argument('--max-fds', type=float, default=2, help='per process')
    parser.add_argument('--max-zend-mib', type=float, default=1, help='per worker')
    parser.add_argument('--control', help="a control engine's report: its growth adds to the limits")
    parser.add_argument('--leak', action='store_true', help='self-test: pass only if a per-request leak is caught')
    parser.add_argument('--output', default='target/reports/soak.json')
    args = parser.parse_args()
    args.warmup = args.duration * .2 if args.warmup is None else args.warmup
    [engine] = parse_engines([args.engine], args.php, args.php_extension)
    report = soak(engine, args)
    write_report(args.output, report)
    for failure in report['failures']:
        print(f'FAIL {failure}')
    if args.leak:
        caught = report['failures'] and all(failure.startswith(('rss ', 'zend ')) for failure in report['failures'])
        print('leak caught' if caught else 'leak NOT caught by the growth check')
        return 0 if caught else 1
    return int(bool(report['failures']))


if __name__ == '__main__':
    sys.exit(main())
