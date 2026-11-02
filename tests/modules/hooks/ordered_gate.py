#!/usr/bin/env python3
"""Differential actual pinned native function bodies with deterministic native IO facades.

This checks policy and native operation ordering, not real reactor/context/liburing
or PHP engine execution. Full translation-unit and original suites are separate.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[3]
import sys
sys.path.insert(0, str(ROOT))
from tools.build_upstream import COMMIT as PIN


def block(text, signature):
    start = text.index(signature)
    opening = text.index('{', start)
    end, depth = opening + 1, 1
    while depth:
        if end >= len(text):
            raise ValueError('Unbalanced body '+signature)
        depth += (text[end] == '{') - (text[end] == '}')
        end += 1
    return text[start:end]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, required=True)
    p.add_argument('--archive', type=Path, required=True)
    p.add_argument('--report', type=Path, required=True)
    p.add_argument('--sanitize', action='store_true')
    a = p.parse_args()
    signatures = {
        'src/coroutine/hook.cc': ['static sw_inline bool is_no_coro()'],
        'src/coroutine/system.cc': ['bool wait_for(', 'static inline void event_waiter_callback('],
        'src/coroutine/socket.cc': ['ssize_t Socket::recvmsg(', 'ssize_t Socket::sendmsg(', 'ssize_t Socket::sendto(', 'ssize_t Socket::recvfrom(void *_buf, size_t _n, sockaddr *', 'ssize_t Socket::recv(', 'ssize_t Socket::send(', 'ssize_t Socket::read(', 'ssize_t Socket::write(', 'ssize_t Socket::readv(', 'ssize_t Socket::writev(', 'ssize_t Socket::recv_all(', 'ssize_t Socket::send_all(', 'bool Socket::TimerController::start()', 'Socket::TimerController::~TimerController()'],
        'src/coroutine/uring_socket.cc': ['ssize_t UringSocket::recv_all(', 'ssize_t UringSocket::send_all('],
    }
    signatures['src/reactor/poll.cc'] = ['int16_t translate_events_to_poll(', 'int translate_events_from_poll(']
    manifest = json.loads((ROOT/'migration/patches/hooks/manifest.json').read_text())
    originals = {path: subprocess.check_output(['git', '-C', str(a.source), 'show', PIN+':'+path], text=True) for path in [*manifest['patches'][0]['upstream_paths'], 'src/reactor/poll.cc']}
    with tempfile.TemporaryDirectory(prefix='swr-hooks-order-') as temp:
        tree = Path(temp)
        for path, text in originals.items():
            output = tree/path
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(text)
        for entry in manifest['patches']:
            patch = ROOT/'migration/patches/hooks'/entry['path']
            assert hashlib.sha256(patch.read_bytes()).hexdigest() == entry['sha256']
            subprocess.run(['git', 'apply', str(patch)], cwd=tree, check=True)
        for variant in ['original', 'selected']:
            bodies = []
            for path, names in signatures.items():
                text = originals[path] if variant == 'original' else (tree/path).read_text()
                bodies.extend(block(text, name) for name in names)
                # The selected socket's transfers run through one Rust-driven member template (#443).
                if variant == 'selected' and path == 'src/coroutine/socket.cc':
                    bodies.insert(len(bodies) - len(names), block(text, 'template <class Call>\nssize_t Socket::transfer('))
            (tree/(variant+'.inc')).write_text('\n'.join(bodies))
        command = ['c++', '-std=c++14', '-O1', '-g', '-I'+str(tree), '-I'+str(ROOT/'native/hooks'), str(ROOT/'tests/modules/hooks/ordered_gate.cc'), str(a.archive.resolve()), '-o', str(tree/'ordered'), '-lpthread']
        if a.sanitize:
            command.extend(['-fsanitize=address,undefined', '-fno-omit-frame-pointer'])
        if __import__('sys').platform.startswith('linux'):
            command.extend(['-ldl', '-lm'])
        build = subprocess.run(command, capture_output=True, text=True)
        run = subprocess.run([str(tree/'ordered')], capture_output=True, text=True) if build.returncode == 0 else build
        print(build.stderr, end='')
        print(run.stdout, end='')
        if run is not build:
            print(run.stderr, end='')
        report = dict(pin=PIN, scope=__doc__, manifest=manifest,
                      command=command, compile_returncode=build.returncode, compile_stderr=build.stderr, returncode=run.returncode, stdout=run.stdout, stderr=run.stderr)
        a.report.parent.mkdir(parents=True, exist_ok=True)
        a.report.write_text(json.dumps(report, indent=2)+'\n')
        raise SystemExit(run.returncode)


if __name__ == '__main__':
    main()
