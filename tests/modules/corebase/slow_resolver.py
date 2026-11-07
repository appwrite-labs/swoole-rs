#!/usr/bin/env python3
"""Run a command against a local DNS server whose latency we control (Linux, root).

The command gets new mount and network namespaces: only loopback, with
/etc/resolv.conf naming a server on 127.0.0.1:53 and nsswitch.conf resolving
hosts through files and DNS alone. The server answers each name in --answer at
once (A 127.0.0.1) and every other name NXDOMAIN after --delay seconds. The
command runs as the invoking sudo user. Example, the original System DNS
timeout case on c-ares builds (#1190):

  sudo python3 tests/modules/corebase/slow_resolver.py -- python3 corpus/upstream/run.py \\
    --manifest tests/modules/corebase/dns-timeout-originals.json --external-network ...
"""
import argparse
import os
import socket
import struct
import subprocess
import sys
import tempfile
import threading
from pathlib import Path


def question(packet):
    """The question section's end offset and its name, type."""
    offset, labels = 12, []
    while packet[offset]:
        labels.append(packet[offset + 1:offset + 1 + packet[offset]].decode('ascii', 'replace'))
        offset += 1 + packet[offset]
    qtype = struct.unpack('!H', packet[offset + 1:offset + 3])[0]
    return offset + 5, '.'.join(labels).lower(), qtype


def reply(packet, answer):
    end, _, qtype = question(packet)
    record = struct.pack('!HHHIH4s', 0xC00C, 1, 1, 60, 4, socket.inet_aton('127.0.0.1')) if answer and qtype == 1 else b''
    flags = 0x8180 | (0 if answer else 3)
    return packet[:2] + struct.pack('!HHHHH', flags, 1, 1 if record else 0, 0, 0) + packet[12:end] + record


def serve(server, answers, delay):
    while True:
        packet, peer = server.recvfrom(512)
        try:
            _, name, _ = question(packet)
        except (IndexError, struct.error):
            continue
        response = reply(packet, name in answers)
        threading.Timer(0 if name in answers else delay, server.sendto, (response, peer)).start()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--answer', action='append', default=['www.github.com'], help='Name answered at once')
    parser.add_argument('--delay', type=float, default=0.5, help='Seconds before any other name gets NXDOMAIN')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('no command')
    os.unshare(os.CLONE_NEWNS | os.CLONE_NEWNET)
    subprocess.run(['mount', '--make-rprivate', '/'], check=True)
    subprocess.run(['ip', 'link', 'set', 'lo', 'up'], check=True)
    config = Path(tempfile.mkdtemp(prefix='slow-resolver-'))
    (config / 'resolv.conf').write_text('nameserver 127.0.0.1\noptions timeout:5 attempts:1\n')
    (config / 'nsswitch.conf').write_text('hosts: files dns\n')
    for name in ['resolv.conf', 'nsswitch.conf']:
        subprocess.run(['mount', '--bind', str(config / name), f'/etc/{name}'], check=True)
    server = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    server.bind(('127.0.0.1', 53))
    threading.Thread(target=serve, args=(server, {name.lower() for name in args.answer}, args.delay), daemon=True).start()
    user = {'user': int(os.environ['SUDO_UID']), 'group': int(os.environ['SUDO_GID']), 'extra_groups': []} if 'SUDO_UID' in os.environ else {}
    return subprocess.run(command, **user).returncode


if __name__ == '__main__':
    sys.exit(main())
