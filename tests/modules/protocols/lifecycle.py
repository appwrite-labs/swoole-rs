#!/usr/bin/env python3
"""Exercise protocol worker IPC and graceful PHP shutdown with real allocators."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import socket
import struct
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, parse_engines, server, write_report
from network import mqtt, read


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/protocols-lifecycle.json')
    args = parser.parse_args()
    script = Path(__file__).with_name('server.php')
    report = {'created_utc': datetime.now(timezone.utc).isoformat(),
              'scope': 'Real PHP MQTT/RESP servers, concurrent2MiB worker IPC, response hashes, graceful master/worker shutdown and worker-error callback. No OOM injection or memory-leak proof.',
              'engines': [], 'results': []}
    for engine in parse_engines(args.engine, args.php, args.php_extension):
        report['engines'].append(engine.metadata())
        for protocol in ['mqtt', 'resp']:
            for mode in ['base', 'process']:
                outcome = {'engine': engine.name, 'protocol': protocol, 'mode': mode, 'passed': False}
                with tempfile.TemporaryDirectory(prefix='swr-protocols-shutdown-') as directory:
                    root = Path(directory)
                    errors, log = root / 'worker-errors', root / 'server.log'
                    shutdown = mqtt(b'SHUTDOWN') if protocol == 'mqtt' else b'*1\r\n$8\r\nSHUTDOWN\r\n'
                    env = {'TEST_PROTOCOL': protocol, 'TEST_SERVER_MODE': mode,
                           'TEST_SHUTDOWN_SHA256': hashlib.sha256(shutdown).hexdigest(),
                           'TEST_WORKER_ERRORS': str(errors), 'TEST_LOG_FILE': str(log)}
                    try:
                        with server(engine, script, env) as (port, proc):
                            def request(index):
                                payload = bytes([index]) * (2 * 1024 * 1024)
                                wire = mqtt(payload) if protocol == 'mqtt' else b'*1\r\n$' + str(len(payload)).encode() + b'\r\n' + payload + b'\r\n'
                                with socket.create_connection(('127.0.0.1', port), timeout=5) as client:
                                    client.settimeout(5)
                                    client.sendall(wire)
                                    assert read(client, 36) == struct.pack('!I', len(wire)) + hashlib.sha256(wire).digest()
                                return hashlib.sha256(wire).hexdigest()
                            with ThreadPoolExecutor(max_workers=4) as pool:
                                outcome['payload_sha256'] = list(pool.map(request, range(8)))
                            with socket.create_connection(('127.0.0.1', port), timeout=5) as client:
                                client.settimeout(5)
                                client.sendall(shutdown)
                                assert read(client, 36) == struct.pack('!I', len(shutdown)) + hashlib.sha256(shutdown).digest()
                            assert proc.wait(timeout=10) == 0, 'Master did not exit cleanly'
                            outcome['master_exit_code'] = proc.returncode
                        outcome['worker_errors'] = errors.read_text() if errors.is_file() else ''
                        assert not outcome['worker_errors'], 'Worker crashed: ' + outcome['worker_errors']
                        outcome['passed'] = True
                    except Exception as error:
                        outcome['error'] = f'{type(error).__name__}: {error}'
                    outcome['server_log'] = log.read_text() if log.is_file() else ''
                report['results'].append(outcome)
                write_report(args.output, report)
    print(json.dumps({'passed': sum(row['passed'] for row in report['results']), 'total': len(report['results'])}))
    return int(any(not row['passed'] for row in report['results']))


if __name__ == '__main__':
    sys.exit(main())
