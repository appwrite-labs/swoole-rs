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

--bailout CASE instead sends --requests requests to /bail/CASE, each ending in a bailout
(exit() with coroutines suspended, a fatal error, or a fatal error while a coroutine is
parked in a hooked read), and fits the RSS of the processes alive throughout against the
requests sent: a bailout ends its worker on both engines, which the manager restarts, so
what one could leak lives on only in the master and manager (#5283). With --control it
fails when this engine grows more per request than the control plus
--max-rss-per-request.

--threads N (a thread-safe PHP) runs threads.php: N threads started and joined, each
adding and deleting an event and registering a signal callback, then a thread-mode
server whose worker thread, with a port callback on a second listener, is restarted N
times with reload(). It reports RSS growth per thread and per reload, and with --control
fails when either exceeds the control's plus --max-rss-per-request (#5285).
"""
import argparse
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.harness import parse_engines, run_isolated, server, stop_process_group, temp_file_count, write_report
from corpus.run import WebSocket

HERE = Path(__file__).resolve().parent
MIB = 1024 * 1024
POOL_WORKERS = 2


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


def leftover_growth(samples, metric):
    """Compare low-water marks in the first and final thirds of the measured window."""
    if metric == 'live_processes':
        # A normal pool worker can be between exit and respawn at either endpoint.
        values = [s[metric] + max(0, POOL_WORKERS - s['pool_workers']) for s in samples]
    else:
        values = [s[metric] for s in samples]
    window = max(1, len(samples) // 3)
    return min(values[-window:]) - min(values[:window])


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


def soak(engine, args, tmpdir):
    env = {'TMPDIR': tmpdir, 'SOAK_LEAK': '1' if args.leak else '0', 'SOAK_REDIS': args.redis or '', 'SOAK_MYSQL': args.mysql or ''}
    pool = subprocess.Popen(engine.command(HERE / 'pool.php', str(POOL_WORKERS)), stdout=subprocess.DEVNULL, start_new_session=True,
                            env={**os.environ, **env})
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
                workers = tree.get(pool.pid, [])
                pool_children.update(workers)
                sample = {'t': round(now, 1), 'processes': {}, 'temp_files': temp_file_count(tmpdir),
                          'pool_workers': len(workers),
                          'live_processes': len(descendants(proc.pid, tree)) + len(descendants(pool.pid, tree))}
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
    if len(measured) >= 3:
        for metric in ('temp_files', 'live_processes'):
            change = leftover_growth(measured, metric)
            growths[metric] = change
            limit = max(0, control['growth'].get(metric, 0)) if args.control else 0
            if change > limit:
                failures.append(f'{metric} grew {change} after warm-up (limit {limit})')
    if len(measured) < 3:
        failures.append(f'only {len(measured)} samples after warm-up')
    return {'engine': engine.metadata(), 'duration': args.duration, 'warmup': args.warmup,
            'limits': limits, 'growth': growths, 'failures': failures,
            'requests': load.counts if load else {}, 'pool_workers_started': len(pool_children) if load else 0,
            'samples': samples}


def bailout(engine, args, tmpdir):
    """Requests to /bail/CASE: the process tree's RSS growth per request."""
    samples, failures, workers = [], [], set()
    path = f'/bail/{args.bailout}'
    every = max(1, args.requests // 20)
    try:
        with server(engine, HERE / 'server.php', {'TMPDIR': tmpdir}) as (port, proc):
            def send(count):
                failed = 0
                for _ in range(count):
                    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
                    try:
                        connection.request('GET', path)
                        response = connection.getresponse()
                        if response.status != 200 or response.read() != b'ok':
                            failed += 1
                    except (OSError, http.client.HTTPException):
                        failed += 1
                    finally:
                        connection.close()
                return failed
            # Warm-up: the first restarts settle the manager's and master's buffers. Its
            # requests are left out of the fit, not of the verdict.
            warmup = min(200, args.requests)
            failed = send(warmup)
            sent = 0
            while sent < args.requests:
                batch = min(every, args.requests - sent)
                failed += send(batch)
                sent += batch
                if proc.poll() is not None:
                    raise AssertionError(f'server exited {proc.returncode} after {sent} requests')
                tree = children()
                pids = descendants(proc.pid, tree)
                workers.update(pids)
                rss = {pid: measured[0] for pid in pids if (measured := resources(pid)) is not None}
                samples.append({'requests': sent, 'rss': rss})
                print(json.dumps({'requests': sent, 'processes': len(rss)}), flush=True)
        if failed:
            failures.append(f'{failed} of {warmup + sent} requests got no 200 ok')
    except Exception as error:
        failures.append(f'{type(error).__name__}: {error}')
    per_request = None
    # Only the processes alive at every sample, the master and manager: each bailout ends
    # its worker, so a worker's RSS is a restart's, not a leak's.
    lasting = set.intersection(*(set(s['rss']) for s in samples)) if samples else set()
    if len(samples) >= 3 and lasting:
        points = [(s['requests'], sum(s['rss'][pid] for pid in lasting)) for s in samples]
        per_request = growth(points) / (points[-1][0] - points[0][0])
        limit = args.max_rss_per_request
        if args.control:
            limit += max(0, json.loads(Path(args.control).read_text()).get('rss_per_request') or 0)
            if per_request > limit:
                failures.append(f'rss grew {per_request:.0f} bytes per request (limit {limit:.0f})')
    else:
        failures.append(f'only {len(samples)} samples, {len(lasting)} processes alive at each')
    return {'engine': engine.metadata(), 'bailout': args.bailout, 'requests': args.requests,
            'rss_per_request': per_request, 'lasting_processes': len(lasting), 'processes_seen': len(workers),
            'failures': failures, 'samples': [{'requests': s['requests'], 'rss': sum(s['rss'].values())} for s in samples]}


def threads(engine, args, tmpdir):
    """threads.php's churn and reload cases: RSS growth per thread and per reload."""
    failures, report = [], {'engine': engine.metadata(), 'threads': args.threads}
    # Not under the soak's TMPDIR: a thread's RINIT reads it, which is its own case.
    churn = run_isolated(engine.command(HERE / 'threads.php', str(args.threads)), timeout=1800,
                         env={**os.environ, 'SOAK_THREADS': 'churn'})
    try:
        report['per_thread'] = json.loads(churn.stdout.strip().splitlines()[-1])['per_thread']
    except (IndexError, ValueError, KeyError):
        failures.append(f'churn exited {churn.returncode}: {churn.stdout[-300:]} {churn.stderr[-300:]}')
    try:
        # The second listener's port: fixed, as each worker thread runs the script again.
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            extra = probe.getsockname()[1]
        env = {'SOAK_THREADS': 'server', 'SOAK_EXTRA_PORT': str(extra)}
        with server(engine, HERE / 'threads.php', env) as (port, proc):
            def get(path):
                connection = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
                try:
                    connection.request('GET', path)
                    response = connection.getresponse()
                    return response.status, response.read()
                finally:
                    connection.close()

            def rss():
                for _ in range(500):
                    try:
                        status, body = get('/rss')
                        if status == 200:
                            return int(body)
                    except (OSError, http.client.HTTPException, ValueError):
                        pass
                    time.sleep(.01)
                raise AssertionError('no /rss answer')

            def reload():
                # The worker thread answering may be the one restarting: a reset is expected.
                try:
                    get('/reload')
                except (OSError, http.client.HTTPException):
                    pass
                # The restarted worker thread answers again.
                for _ in range(500):
                    try:
                        if get('/')[0] == 200:
                            return
                    except (OSError, http.client.HTTPException):
                        pass
                    time.sleep(.01)
                raise AssertionError('the worker thread did not come back after reload()')
            for _ in range(20):
                reload()
            start = rss()
            for _ in range(args.threads):
                reload()
            report['per_reload'] = (rss() - start) / args.threads
    except Exception as error:
        failures.append(f'{type(error).__name__}: {error}')
    if args.control:
        control = json.loads(Path(args.control).read_text())
        for metric in ('per_thread', 'per_reload'):
            if metric in report and metric in control:
                limit = max(0, control[metric]) + args.max_rss_per_request
                if report[metric] > limit:
                    failures.append(f'rss grew {report[metric]:.0f} bytes {metric.replace("_", " ")} (limit {limit:.0f})')
    report['failures'] = failures
    return report


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
    parser.add_argument('--bailout', choices=['exit', 'fatal', 'read'], help='requests that end in this bailout instead of the load')
    parser.add_argument('--requests', type=int, default=5000, help='--bailout: requests to send')
    parser.add_argument('--threads', type=int, help='threads to start and join, and reloads, on a thread-safe PHP')
    parser.add_argument('--max-rss-per-request', type=float, default=256,
                        help="--bailout, --threads: bytes per request, thread or reload over the control's")
    parser.add_argument('--output', default='target/reports/soak.json')
    args = parser.parse_args()
    args.warmup = args.duration * .2 if args.warmup is None else args.warmup
    [engine] = parse_engines([args.engine], args.php, args.php_extension)
    with tempfile.TemporaryDirectory(prefix='swoole-soak-') as tmpdir:
        if args.threads:
            report = threads(engine, args, tmpdir)
        elif args.bailout:
            report = bailout(engine, args, tmpdir)
        else:
            report = soak(engine, args, tmpdir)
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
