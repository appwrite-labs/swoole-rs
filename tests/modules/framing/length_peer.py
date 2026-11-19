"""Pin the bytes and closes each length-framing policy exposes on Linux TCP."""
import argparse
from pathlib import Path
import socket
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import parse_engines, server


def check(engine, case, data, expected_close, expected_trace):
    with tempfile.TemporaryDirectory() as directory:
        trace = Path(directory) / 'callbacks'
        settings = {'TEST_CASE': case, 'TEST_TRACE': str(trace)}
        with server(engine, Path(__file__).with_suffix('.php'), settings) as (port, _):
            with socket.create_connection(('127.0.0.1', port), timeout=5) as peer:
                peer.sendall(data)
                try:
                    reply = peer.recv(1)
                    close = 'fin' if not reply else f'data {reply!r}'
                except ConnectionResetError:
                    close = 'reset'
        callbacks = trace.read_text().splitlines() if trace.exists() else []
    if (close, callbacks) != (expected_close, expected_trace):
        raise AssertionError(f'{engine.name} {case}: got {close}, {callbacks}, '
                             f'expected {expected_close}, {expected_trace}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    args = parser.parse_args()
    engines = parse_engines(args.engine, args.php, args.php_extension)
    if {engine.name for engine in engines} != {'upstream', 'cargo'}:
        parser.error('supply the upstream and cargo engines')
    first = (1).to_bytes(4, 'big') + b'A'
    second = (1).to_bytes(4, 'big') + b'B'
    over = (2**20).to_bytes(4, 'big')
    for engine in engines:
        greedy = engine.name == 'cargo'
        check(engine, 'single', first, 'fin', [first.hex()])
        check(engine, 'coalesced', first + second, 'fin' if greedy else 'reset', [first.hex()])
        check(engine, 'callback', b'hello', 'reset', ['5' if greedy else '4'])
        check(engine, 'limit-header', over, 'fin', [])
        check(engine, 'limit-with-body', over + b'hello', 'reset', [])


if __name__ == '__main__':
    main()
