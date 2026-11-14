#!/usr/bin/env python3
"""Memory of a server's processes (master, manager, workers), Cargo build vs upstream.

For each workload, every engine's server starts in turn, serves --warmup checked requests,
and then has every process's /proc/<pid>/smaps_rollup read at three points:

  idle   after the warmup
  after  once --requests more requests have been served on one connection: growth over
         idle that scales with requests is retained memory
  held   while --connections clients each also hold a connection open after one request
         (a WebSocket after one echo; HTTP/2 after the preface): the growth over `after`
         is the per-connection cost

PSS splits the pages the forked processes share, so its sum over the processes is the
server's footprint; USS (private pages) is what the processes cost on their own; RSS counts
shared pages once per process. PSS and USS also split file pages with any other process
mapping the same files (another PHP), so engines compare on RSS and dirty pages. Dirty (private dirty pages: heap, stacks, written data) is
what connections and requests grow, and the kernel cannot reclaim it the way it drops clean
code pages, so the per-connection and per-request figures use it. Values are KiB, the
median over --repetitions. Linux only.
"""
import argparse
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from benchmarks.callgrind import WORKLOADS, h2_frame, send, wait_for_port
from benchmarks.wall import session_pids
from tools.harness import ROOT, parse_engines
from corpus.run import WebSocket

FIELDS = {'Pss': ['pss'], 'Rss': ['rss'], 'Private_Clean': ['uss'], 'Private_Dirty': ['uss', 'dirty']}
POINTS = ('idle', 'after', 'held')


def snapshot(sid):
    """{'processes': n, 'pss'/'uss'/'rss'/'dirty': KiB summed over session `sid`'s processes}."""
    total = {'processes': 0, 'pss': 0, 'uss': 0, 'rss': 0, 'dirty': 0}
    for pid in session_pids(sid):
        try:
            lines = Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines()
        except OSError:
            continue  # exited meanwhile
        total['processes'] += 1
        for line in lines:
            name, _, value = line.partition(':')
            for key in FIELDS.get(name, ()):
                total[key] += int(value.split()[0])
    return total


def hold(port, workload):
    """One open connection that has done one exchange; close() releases it."""
    _, method, path, payload, expected = workload
    if method == 'WS':
        client = WebSocket(port)
        client.send(payload)
        if client.receive() != expected:
            raise RuntimeError('unexpected WebSocket frame')
        return client
    if method == 'H2':
        sock = socket.create_connection(('127.0.0.1', port), timeout=60)
        sock.sendall(b'PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n' + h2_frame(4, 0, 0))
        sock.recv(9)  # the server's SETTINGS: the session exists
        return sock
    conn = http.client.HTTPConnection('127.0.0.1', port, timeout=60)
    conn.request(method, path, payload)
    response = conn.getresponse()
    if response.read() != expected:
        raise RuntimeError(f'unexpected response {response.status}')
    return conn


def settled(sid):
    time.sleep(0.5)  # let the workers finish what the last exchange started
    return snapshot(sid)


def measure(engine, workload, connections, requests, warmup):
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    with tempfile.TemporaryDirectory() as data:
        proc = subprocess.Popen(engine.command(ROOT / WORKLOADS[workload][0] / 'server.php'),
                                env={**os.environ, 'TEST_PORT': str(port), 'TEST_DATA': data},
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            wait_for_port(port, proc)
            send(port, WORKLOADS[workload], warmup)
            idle = settled(proc.pid)
            send(port, WORKLOADS[workload], requests)
            after = settled(proc.pid)
            held = [hold(port, WORKLOADS[workload]) for _ in range(connections)]
            loaded = settled(proc.pid)
            for conn in held:
                conn.close()
        finally:
            os.killpg(proc.pid, signal.SIGTERM)
            proc.wait(timeout=60)
    return {'idle': idle, 'after': after, 'held': loaded}


def median(runs):
    return {point: {key: statistics.median(run[point][key] for run in runs) for key in runs[0][point]}
            for point in POINTS}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--engine', action='append', required=True, help='NAME=/path/to/extension')
    parser.add_argument('--workload', choices=list(WORKLOADS), action='append')
    parser.add_argument('--repetitions', type=int, default=3)
    parser.add_argument('--connections', type=int, default=256)
    parser.add_argument('--requests', type=int, default=5000)
    parser.add_argument('--warmup', type=int, default=200)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', type=Path,
                        default=ROOT / 'target' / 'memory' / time.strftime('%Y%m%dT%H%M%S') / 'memory.json')
    args = parser.parse_args()
    engines = parse_engines(args.engine, args.php, args.php_extension)
    results = {}
    for workload in args.workload or ['plaintext']:
        runs = {engine.name: [] for engine in engines}
        for _ in range(args.repetitions):
            for engine in engines:  # interleaved, as wall.py
                runs[engine.name].append(measure(engine, workload, args.connections, args.requests, args.warmup))
        results[workload] = {name: {'runs': rows, 'median': median(rows)} for name, rows in runs.items()}
        for name, result in results[workload].items():
            m = result['median']
            per_conn = (m['held']['dirty'] - m['after']['dirty']) * 1024 / args.connections
            per_req = (m['after']['dirty'] - m['idle']['dirty']) * 1024 / args.requests
            print(f'{name:10} {workload:15} processes {m["idle"]["processes"]:.0f}  idle KiB: pss '
                  f'{m["idle"]["pss"]:.0f} uss {m["idle"]["uss"]:.0f} rss {m["idle"]["rss"]:.0f} '
                  f'dirty {m["idle"]["dirty"]:.0f}  dirty B/connection {per_conn:.0f}  '
                  f'dirty B/request {per_req:+.1f}', flush=True)
        if len(engines) == 2:
            first, second = (results[workload][engine.name]['median'] for engine in engines)
            print(f'ratio {engines[0].name}/{engines[1].name} {workload} idle rss '
                  f'{first["idle"]["rss"] / second["idle"]["rss"]:.4f} dirty '
                  f'{first["idle"]["dirty"] / second["idle"]["dirty"]:.4f}', flush=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({'connections': args.connections, 'requests': args.requests,
                                       'workloads': results}, indent=2) + '\n')
    print('results: ' + str(args.output))
    return 0


if __name__ == '__main__':
    sys.exit(main())
