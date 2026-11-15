#!/usr/bin/env python3
"""Wall-clock throughput, latency and perf counters per request, Cargo build vs upstream (#1906).

Each repetition starts every engine's server in turn (interleaved), checks the response,
warms it up, then drives it for --seconds at fixed concurrency with wrk (HTTP/1), h2load
(HTTP/2) or a WebSocket echo loop, while `perf stat` counts the server's processes. Reports
the median and spread of requests/s, p50/p99 latency and counters per request. Timings on
shared runners are noisy, so --max-ratio judges a counter ratio (first engine over second),
never wall time, and exits 1 when the median ratio of any workload exceeds it.
"""
import argparse
import json
import os
from pathlib import Path
import re
import resource
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from benchmarks.callgrind import WORKLOADS, send, wait_for_port
from tools.harness import ROOT, parse_engines
from corpus.run import WebSocket

EVENTS = ['task-clock', 'cycles', 'instructions', 'branch-misses', 'cache-misses']
UNITS = {'us': 1e-3, 'ms': 1, 's': 1e3}


def session_pids(sid):
    """Every process in session `sid`: the server's master, manager and workers."""
    pids = []
    for stat in Path('/proc').glob('[0-9]*/stat'):
        try:
            fields = stat.read_text().rsplit(')', 1)[1].split()
        except OSError:
            continue
        if int(fields[3]) == sid:
            pids.append(stat.parent.name)
    return pids


def wrk(port, path, connections, seconds):
    out = subprocess.run(['wrk', '--latency', '-t2', f'-c{connections}', f'-d{seconds}s',
                          f'http://127.0.0.1:{port}{path}'], check=True, capture_output=True, text=True).stdout
    if 'Non-2xx' in out or 'Socket errors' in out:
        raise RuntimeError(out)
    latency = {p: float(v) * UNITS[u] for p, v, u in re.findall(r'^\s+(50|99)%\s+([\d.]+)(us|ms|s)$', out, re.M)}
    return int(re.search(r'(\d+) requests in', out)[1]), latency['50'], latency['99']


def h2load(port, path, connections, seconds):
    with tempfile.NamedTemporaryFile('r') as log:
        out = subprocess.run(['h2load', f'-c{connections}', '-m1', f'-D{seconds}', f'--log-file={log.name}',
                              f'http://127.0.0.1:{port}{path}'], check=True, capture_output=True, text=True).stdout
        times = sorted(int(line.split()[-1]) / 1e3 for line in log)
    done = re.search(r'(\d+) succeeded, (\d+) failed, (\d+) errored', out)
    if not done or done[2] != '0' or done[3] != '0':
        raise RuntimeError(out)
    return int(done[1]), percentile(times, 50), percentile(times, 99)


def websocket(port, payload, seconds, pipeline=32):
    # One connection: 06-websocket broadcasts to every client. Bounded batches keep multiple
    # messages in flight without changing that fixture into a fan-out workload (#3409).
    if pipeline < 1 or len(payload) > 125:
        raise ValueError('WebSocket wall load needs a positive pipeline and a short payload')
    client, times = WebSocket(port), []
    mask = os.urandom(4)
    frame = bytes([0x81, 0x80 | len(payload)]) + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
    echo = bytes([0x81, len(payload)]) + payload
    frame, expected = frame * pipeline, echo * pipeline
    sendall, recv, clock = client.socket.sendall, client.socket.recv, time.perf_counter
    deadline = time.monotonic() + seconds
    try:
        while time.monotonic() < deadline:
            start = clock()
            sendall(frame)
            received = 0
            while received < len(expected):
                got = recv(len(expected) - received)
                if not got or got != expected[received:received + len(got)]:
                    raise RuntimeError(f'unexpected WebSocket echo {got!r}')
                complete = (received + len(got)) // len(echo) - received // len(echo)
                times.extend([(clock() - start) * 1e3] * complete)
                received += len(got)
    finally:
        client.close()
    times.sort()
    return len(times), percentile(times, 50), percentile(times, 99)


def percentile(sorted_values, p):
    return sorted_values[min(len(sorted_values) - 1, len(sorted_values) * p // 100)]


def run(engine, workload, connections, seconds, warmup, websocket_pipeline=32):
    directory, method, path, payload = WORKLOADS[workload][:4]
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    with tempfile.TemporaryDirectory() as data, tempfile.NamedTemporaryFile('r') as counts:
        proc = subprocess.Popen(engine.command(ROOT / directory / 'server.php'),
                                env={**os.environ, 'TEST_PORT': str(port), 'TEST_DATA': data},
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            wait_for_port(port, proc)
            send(port, WORKLOADS[workload], warmup)  # also checks every response body
            perf = subprocess.Popen(['perf', 'stat', '-x,', '-e', ','.join(EVENTS), '-o', counts.name,
                                     '-p', ','.join(session_pids(proc.pid))])
            time.sleep(0.2)  # perf attaches before the load starts
            start, client = time.monotonic(), client_seconds()
            if method == 'WS':
                requests, p50, p99 = websocket(port, payload, seconds, websocket_pipeline)
            elif method == 'H2':
                requests, p50, p99 = h2load(port, path, connections, seconds)
            elif method == 'GET':
                requests, p50, p99 = wrk(port, path, connections, seconds)
            else:
                raise ValueError(f'{workload}: no load generator for {method}')
            elapsed, client = time.monotonic() - start, client_seconds() - client
            perf.send_signal(signal.SIGINT)
            perf.wait(timeout=30)
        finally:
            os.killpg(proc.pid, signal.SIGTERM)
            proc.wait(timeout=60)
        counters = {}
        for line in counts:
            fields = line.split(',')
            if len(fields) > 2 and fields[2] in EVENTS:
                # '<not supported>' or '<not counted>' where the runner exposes no PMU.
                counters[fields[2]] = float(fields[0]) / requests if fields[0][:1].isdigit() else None
    return {'requests_per_second': requests / elapsed, 'p50_ms': p50, 'p99_ms': p99,
            'client_cpu': client / elapsed, 'per_request': counters}


def client_seconds():
    """CPU seconds of this process and its reaped children: the load generator's, since the
    server and perf are reaped after the load. Near 1 (or wrk's two threads' 2), requests/s
    measures the client rather than the server."""
    own, children = resource.getrusage(resource.RUSAGE_SELF), resource.getrusage(resource.RUSAGE_CHILDREN)
    return own.ru_utime + own.ru_stime + children.ru_utime + children.ru_stime


def summary(rows):
    """Median and relative spread ((max - min) / median) of each measure across the repetitions
    that counted it; one lost perf sample doesn't drop a counter, a PMU that counts nothing does."""
    measures = {key: [row.get(key) for row in rows] for key in ('requests_per_second', 'p50_ms', 'p99_ms', 'client_cpu')}
    measures |= {event: [row['per_request'].get(event) for row in rows] for event in EVENTS}
    result = {}
    for key, values in measures.items():
        if values := [value for value in values if value is not None]:
            median = statistics.median(values)
            result[key] = {'median': median, 'spread': (max(values) - min(values)) / median if median else 0,
                           'samples': len(values)}
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True, help='NAME=/path/to/extension')
    parser.add_argument('--workload', choices=list(WORKLOADS), action='append')
    parser.add_argument('--repetitions', type=int, default=5)
    parser.add_argument('--seconds', type=int, default=5)
    parser.add_argument('--connections', type=int, default=32)
    parser.add_argument('--websocket-pipeline', type=int, default=32,
                        help='Messages per batch on the single WebSocket connection; 1 restores serial round trips')
    parser.add_argument('--warmup', type=int, default=200)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--counter', choices=EVENTS, default='task-clock',
                        help='The counter --max-ratio judges; GitHub-hosted runners count no hardware events')
    parser.add_argument('--max-ratio', type=float, help='Fail if the first/second median --counter per request '
                        'exceeds this for any workload')
    parser.add_argument('--output', type=Path,
                        default=ROOT / 'target' / 'wall' / time.strftime('%Y%m%dT%H%M%S') / 'wall.json')
    args = parser.parse_args()
    if args.websocket_pipeline < 1:
        parser.error('--websocket-pipeline must be positive')
    engines = parse_engines(args.engine, args.php, args.php_extension)
    if args.max_ratio and len(engines) != 2:
        parser.error('--max-ratio needs exactly two engines')
    results, failed = {}, False
    for workload in args.workload or ['plaintext']:
        rows = {engine.name: [] for engine in engines}
        for _ in range(args.repetitions):
            for engine in engines:
                rows[engine.name].append(run(engine, workload, args.connections, args.seconds, args.warmup,
                                            args.websocket_pipeline))
        results[workload] = {name: {'runs': runs, 'summary': summary(runs)} for name, runs in rows.items()}
        for name, result in results[workload].items():
            print(f'{name:10} {workload:15} ' + '  '.join(
                f'{key} {value["median"]:.4g} ±{value["spread"]:.0%}'
                + (f' ({value["samples"]}/{args.repetitions} counted)' if value['samples'] < args.repetitions else '')
                for key, value in result['summary'].items()), flush=True)
        if len(engines) == 2:
            first, second = (results[workload][engine.name]['summary'].get(args.counter) for engine in engines)
            if not first or not second:
                print(f'{workload}: {args.counter} not counted on this machine')
                failed |= bool(args.max_ratio)
                continue
            ratio = first['median'] / second['median']
            over = bool(args.max_ratio and ratio > args.max_ratio)
            failed |= over
            print(f'ratio {engines[0].name}/{engines[1].name} {workload} {args.counter}/request: {ratio:.4f}'
                  + (f' exceeds {args.max_ratio}' if over else ''))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({'seconds': args.seconds, 'connections': args.connections,
                                       'websocket_pipeline': args.websocket_pipeline,
                                       'counter': args.counter, 'max_ratio': args.max_ratio,
                                       'workloads': results}, indent=2) + '\n')
    print('results: ' + str(args.output))
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
