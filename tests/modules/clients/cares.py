#!/usr/bin/env python3
"""Actual system c-ares over a private UDP DNS peer, including query retirement."""
import argparse
from contextlib import contextmanager
import json
from pathlib import Path
import socket
import struct
import sys
import threading

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import parse_engines, run_isolated


@contextmanager
def peer(mode):
    errors = []
    packets = []
    stop = threading.Event()
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server:
        server.bind(('127.0.0.1', 0))
        server.settimeout(0.05)
        def serve():
            while not stop.is_set():
                try:
                    query, address = server.recvfrom(65535)
                    offset = 12
                    while query[offset]:
                        offset += query[offset] + 1
                    offset += 1
                    kind, family = struct.unpack('!HH', query[offset:offset+4])
                    if kind not in (1, 28) or family != 1:
                        raise ValueError('Unexpected actual SDK question')
                    packets.append({'type': kind, 'question': query[12:offset+4].hex()})
                    if mode in ('timeout', 'cancel'):
                        continue
                    question = query[12:offset+4]
                    response = query[:2] + struct.pack('!HHHHH', 0x8183 if mode == 'nxdomain' else 0x8180,
                                                     1, 0 if mode == 'nxdomain' else 1, 0, 0) + question
                    if mode != 'nxdomain':
                        ip = socket.inet_pton(socket.AF_INET6 if kind == 28 else socket.AF_INET,
                                              '2001:db8::42' if kind == 28 else '192.0.2.42')
                        response += b'\xc0\x0c' + struct.pack('!HHIH', kind, 1, 30, len(ip)) + ip
                    server.sendto(response, address)
                except socket.timeout:
                    pass
                except Exception as error:
                    errors.append(str(error))
                    return
        thread = threading.Thread(target=serve)
        thread.start()
        try:
            yield f'127.0.0.1:{server.getsockname()[1]}', packets, errors
        finally:
            stop.set()
            thread.join(timeout=2)
            if thread.is_alive():
                raise RuntimeError('DNS peer did not stop')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--engine', action='append', required=True)
    p.add_argument('--php', default='php')
    p.add_argument('--php-extension', action='append', default=[])
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    report = {'passed': False, 'engines': [], 'backend_info': [], 'cases': []}
    a.output.parent.mkdir(parents=True, exist_ok=True)
    a.output.write_text(json.dumps(report, indent=2) + '\n')
    observations = {}
    for engine in parse_engines(a.engine, a.php, a.php_extension):
        report['engines'].append(engine.metadata())
        info = run_isolated(engine.command('--ri', 'swoole'), timeout=10)
        report['backend_info'].append(dict(engine=engine.name, exit=info.returncode,
                                           stdout=info.stdout, stderr=info.stderr))
        a.output.write_text(json.dumps(report, indent=2) + '\n')
        if info.returncode or info.stderr or 'c-ares =>' not in info.stdout:
            raise ValueError('This witness requires a real c-ares-enabled extension')
        for mode in ('a', 'aaaa', 'immediate', 'concurrent', 'nxdomain', 'timeout', 'cancel'):
            with peer(mode) as (address, packets, errors):
                result = run_isolated(engine.command(Path(__file__).with_suffix('.php'), mode, address), timeout=15)
            row = dict(engine=engine.name, mode=mode, exit=result.returncode, stdout=result.stdout,
                       stderr=result.stderr, packets=packets, peer_errors=errors)
            report['cases'].append(row)
            a.output.write_text(json.dumps(report, indent=2) + '\n')
            if result.returncode or result.stderr or errors or (mode != 'immediate' and not packets):
                print(json.dumps(row), file=sys.stderr)
                return 1
            row['summary'] = json.loads(result.stdout)
            if row['summary']['mode'] != mode:
                raise ValueError('Actual fixture returned a different mode')
            if mode in observations and observations[mode] != result.stdout:
                print(json.dumps(row), file=sys.stderr)
                return 1
            observations[mode] = result.stdout
    report['passed'] = True
    a.output.write_text(json.dumps(report, indent=2) + '\n')
    print(f'7 real c-ares cases passed on {len(report["engines"])} engines')
    return 0


if __name__ == '__main__':
    sys.exit(main())
