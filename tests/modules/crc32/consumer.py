#!/usr/bin/env python3
"""Actual non-IPv4 datagram dispatch through the pinned CRC32 native caller."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys
import zlib


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--extension', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    extension = args.extension.resolve(strict=True)
    manifest = json.loads(args.manifest.read_text())
    assert sha(extension) == manifest['extension_sha256']
    report = {'passed': False, 'platform': sys.platform}
    command = [args.php, '-n']
    for module in args.php_extension:
        command.extend(['-d', 'extension=' + module])
    command.extend(['-d', 'extension=' + str(extension), str(Path(__file__).with_suffix('.php'))])
    try:
        with socket.socket(socket.AF_INET6, socket.SOCK_DGRAM) as reservation:
            reservation.bind(('::1', 0))
            port = reservation.getsockname()[1]
        process = subprocess.Popen([*command, str(port)], stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, start_new_session=True)
        try:
            assert select.select([process.stdout], [], [], 10)[0], 'Server readiness timeout'
            line = process.stdout.readline()
            assert line == 'READY\n', 'Server startup failed: ' + line
            # Pinned Address::get_addr returns a textual thread-local buffer,
            # while the caller supplies sockaddr_in6 length (28), not strlen.
            # This fresh reactor only formats ::1, so its zero-initialized tail
            # is deterministic. Do not silently correct this native policy.
            address_bytes = b'::1' + bytes(25)
            expected = zlib.crc32(address_bytes) % 5
            assert expected != 0, 'Fixture must distinguish a stubbed-zero CRC dispatch'
            rows = []
            for client_id in range(8):
                with socket.socket(socket.AF_INET6, socket.SOCK_DGRAM) as client:
                    client.bind(('::1', 0))
                    client.settimeout(5)
                    for length in (0, 1, 2, 3, 20, 64, 1024, 8192):
                        # A zero-byte datagram is a native receive edge; include
                        # it as a real outcome instead of substituting a byte.
                        payload = bytes((i * 37 + client_id) & 255 for i in range(length))
                        client.sendto(payload, ('::1', port))
                        if length == 0:
                            # Exact pinned recvfrom <= 0 branch never dispatches.
                            # This is a native no-callback witness, not a CRC call.
                            client.settimeout(0.25)
                            try:
                                client.recvfrom(65536)
                            except socket.timeout:
                                rows.append({'length': 0, 'client': client_id, 'outcome': 'not_dispatched'})
                                client.settimeout(5)
                                continue
                            raise AssertionError('Empty datagram unexpectedly dispatched')
                        result, _ = client.recvfrom(65536)
                        assert result[4:] == payload, 'Payload changed'
                        worker = int.from_bytes(result[:4], 'big')
                        assert worker == expected, (worker, expected)
                        rows.append({'length': length, 'client': client_id, 'worker': worker})
            report.update(results=rows, crc_input_hex=address_bytes.hex(), expected_worker=expected, worker_count=5)
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
            try:
                stdout, stderr = process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                stdout, stderr = process.communicate()
            report['server'] = {'argv': [*command, str(port)], 'stdout': stdout,
                                'stderr': stderr, 'returncode': process.returncode}
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'datagrams': len(report['results'])}))


if __name__ == '__main__':
    main()
