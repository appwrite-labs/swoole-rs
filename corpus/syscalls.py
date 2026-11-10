#!/usr/bin/env python3
"""Run the corpus applications on two engines under strace and diff the syscalls they make.

PHP-level assertions can't see socket options, non-blocking flags, epoll registrations or
signal dispositions. Each project runs under `strace -f -ff` on both engines, RUNS times
each. Every traced call becomes a signature: its name, the arguments that carry behaviour
(levels, options, flags, events; never fds, addresses, ports, lengths or buffers) and its
result (ok or the errno). A difference is a signature every run of one engine made and no
run of the other did; one that only some runs make is timing, and is listed, not gated.
Each difference needs an entry in syscalls.json, with a reason, or the run fails.
Linux only: strace is.
"""
import argparse
from dataclasses import dataclass
from fnmatch import fnmatchcase
import json
from pathlib import Path
import re
import shutil
import sys
import tempfile
from collections import Counter

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.harness import ROOT, Engine, parse_engines, write_report

# The arguments kept, by position; the others (fds, buffers, lengths, outputs) are dropped.
# A name absent here keeps no arguments: only that it was called, and its result.
KEEP = {
    'socket': (0, 1, 2), 'socketpair': (0, 1, 2), 'bind': (1,), 'connect': (1,), 'listen': (1,),
    'accept4': (3,), 'setsockopt': (1, 2, 3), 'getsockopt': (1, 2), 'shutdown': (1,),
    'sendto': (3,), 'recvfrom': (3,), 'sendmsg': (2,), 'recvmsg': (2,), 'sendmmsg': (3,), 'recvmmsg': (3,),
    'fcntl': (1, 2), 'ioctl': (1,), 'epoll_create1': (0,), 'epoll_ctl': (1, 3), 'pipe2': (1,),
    'eventfd2': (1,), 'signalfd4': (1, 3), 'timerfd_create': (0, 1), 'dup3': (2,),
    'wait4': (2,), 'kill': (1,), 'tgkill': (2,), 'prctl': (0, 1),
    'rt_sigaction': (0, 1),
}
NAME_ONLY = ('accept', 'close', 'read', 'write', 'readv', 'writev', 'sendfile', 'splice', 'dup', 'dup2',
             'epoll_wait', 'epoll_pwait', 'poll', 'ppoll', 'select', 'pselect6', 'fork', 'vfork')
SYSCALLS = sorted({*KEEP, *NAME_ONLY, 'clone', 'clone3'})

CALL = re.compile(r'(?P<name>\w+)\((?P<args>.*)(?:\)\s+= (?P<result>.*)| <unfinished \.\.\.>\)?)$')
NOISE = [(re.compile(r'0x[0-9a-f]+'), 'ADDR'), (re.compile(r'htons\(\d+\)'), 'htons(PORT)'),
         (re.compile(r'sun_path="[^"]*"'), 'sun_path=PATH'), (re.compile(r', data=\{[^}]*\}'), '')]


def split_args(text):
    """Top-level comma-separated arguments: commas inside brackets or strings don't split."""
    args, depth, quoted, start = [], 0, False, 0
    for index, char in enumerate(text):
        if quoted:
            quoted = char != '"' or text[index - 1] == '\\'
        elif char == '"':
            quoted = True
        elif char in '([{':
            depth += 1
        elif char in ')]}':
            depth -= 1
        elif char == ',' and depth == 0:
            args.append(text[start:index].strip())
            start = index + 1
    tail = text[start:].strip()
    return args + [tail] if tail or args else args


def clean(text):
    for pattern, replacement in NOISE:
        text = pattern.sub(replacement, text)
    return text


def result_class(result):
    """ok, the errno name, or ? for a call that never returned (exit, or a kill mid-call)."""
    if result is None:
        return '?'
    errno = re.match(r'(?:-1|\?) (E[A-Z0-9]+)', result)
    return errno.group(1) if errno else '?' if result.startswith('?') else 'ok'


def signature(line):
    """One strace -ff line as a signature, or None for a line that carries none."""
    line = line.strip()
    if not line:
        return None
    if line.startswith('---'):  # --- SIGCHLD {si_signo=SIGCHLD, ...} ---
        return 'signal ' + line.split()[1]
    if line.startswith('+++'):  # +++ exited with 0 +++, +++ killed by SIGTERM +++
        return clean(line)
    call = CALL.match(line)
    if not call:
        raise ValueError(f'unparsed strace line: {line!r}')
    name, text = call['name'], call['args'].replace('<unfinished ...>', '')
    args = split_args(text)
    kept = [clean(args[index]) for index in KEEP.get(name, ()) if index < len(args)]
    if name in ('clone', 'clone3'):  # flags=..., among stacks, tids and tls pointers
        kept = re.findall(r'flags=[\w|]+', text)[:1]
    if name == 'ioctl' and kept == ['FIONBIO'] and len(args) > 2:
        kept.append(args[2])
    return f'{name}({", ".join(kept)}) = {result_class(call["result"])}'


def signatures(directory):
    """Every signature in the trace files under DIRECTORY, counted."""
    counts = Counter()
    for path in sorted(Path(directory).glob('trace.*')):
        for line in path.read_text(errors='replace').splitlines():
            found = signature(line)
            if found:
                counts[found] += 1
    return counts


def differences(runs):
    """{engine: signatures each of its runs made and no other engine's run did}, and the
    signatures some runs of an engine made and others didn't (timing, not gated)."""
    union = {name: set().union(*map(set, counts)) for name, counts in runs.items()}
    always = {name: set.intersection(*map(set, counts)) for name, counts in runs.items()}
    only = {name: sorted(always[name] - set().union(*(union[other] for other in runs if other != name)))
            for name in runs}
    seen = set().union(*union.values())
    unstable = sorted(seen - set().union(*only.values()) - set.intersection(*always.values()))
    return only, unstable


def explain(project, engine, found, allowed):
    """The allowlist entry that accounts for FOUND, a signature only ENGINE made in PROJECT."""
    return next((entry for entry in allowed if entry['engine'] == engine and fnmatchcase(project, entry.get('project', '*'))
                 and fnmatchcase(found, entry['signature'])), None)


@dataclass(frozen=True)
class Traced(Engine):
    """An engine whose PHP runs under strace, writing one file per process to TRACE.<pid>.
    A ?name is no error where the architecture lacks it (fork, poll and select on arm64)."""
    trace: Path = None

    def command(self, *args):
        return ['strace', '-f', '-ff', '-q', '-s', '0', '--seccomp-bpf', '-e', 'trace=' + ','.join('?' + name for name in SYSCALLS),
                '-o', str(self.trace), '--', *super().command(*args)]


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--engine', action='append', required=True, help='NAME=/path/to/extension; exactly two')
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--project', action='append', help='a corpus project id; default every non-driver project')
    parser.add_argument('--runs', type=int, default=2, help='runs per engine and project')
    parser.add_argument('--allowlist', default=str(ROOT / 'corpus/syscalls.json'))
    parser.add_argument('--output', default=str(ROOT / 'target/reports/syscalls.json'))
    args = parser.parse_args()
    from corpus.run import MANIFEST, run_project
    engines = parse_engines(args.engine, args.php, args.php_extension)
    if len(engines) != 2:
        parser.error('compare exactly two engines')
    if not shutil.which('strace'):
        parser.error('strace is not installed (.devcontainer/install.sh installs it)')
    allowed = json.loads(Path(args.allowlist).read_text())
    projects = [p for p in MANIFEST if p.get('mode') != 'driver' and (not args.project or p['id'] in args.project)]
    report = {'schema_version': 1, 'engines': [e.metadata() for e in engines], 'runs': args.runs, 'projects': []}
    failed, used = False, set()
    for project in projects:
        row = {'project': project['id'], 'counts': {}}
        runs = {engine.name: [] for engine in engines}
        try:
            for _ in range(args.runs):
                for engine in engines:
                    with tempfile.TemporaryDirectory(prefix='swoole-strace-') as directory:
                        run_project(Traced(engine.name, engine.extension, engine.php, engine.php_extensions,
                                           Path(directory) / 'trace'), project)
                        runs[engine.name].append(signatures(directory))
        except Exception as error:
            failed = True
            row['error'] = f'{type(error).__name__}: {error}'
            print(f'{project["id"]:20} error: {row["error"]}', flush=True)
            report['projects'].append(row)
            continue
        only, row['unstable'] = differences(runs)
        row['counts'] = {name: dict(sum(counts, Counter())) for name, counts in runs.items()}
        row['differences'] = []
        for engine, found in ((engine, found) for engine, signatures_ in only.items() for found in signatures_):
            entry = explain(project['id'], engine, found, allowed)
            row['differences'].append({'engine': engine, 'signature': found, 'reason': entry and entry['reason']})
            if entry:
                used.add(allowed.index(entry))
            else:
                failed = True
                print(f'{project["id"]:20} only {engine}: {found}', flush=True)
        # Calls both engines make, but more than twice per run and a quarter more often on one:
        # extra calls per request, or fewer. Timing moves counts too, so they're listed, not gated.
        first, second = (row['counts'][engine.name] for engine in engines)
        row['count_differences'] = {found: [first.get(found, 0), second.get(found, 0)] for found in sorted({*first, *second})
                                    if abs(first.get(found, 0) - second.get(found, 0))
                                    > max(2 * args.runs, max(first.get(found, 0), second.get(found, 0)) / 4)}
        for found, (a, b) in row['count_differences'].items():
            print(f'{project["id"]:20} count {engines[0].name} {a}, {engines[1].name} {b}: {found}', flush=True)
        explained = sum(bool(d['reason']) for d in row['differences'])
        print(f'{project["id"]:20} {len(row["differences"]) - explained} unexplained, {explained} allowed, '
              f'{len(row["unstable"])} unstable', flush=True)
        report['projects'].append(row)
    if len(projects) == len([p for p in MANIFEST if p.get('mode') != 'driver']):
        report['stale'] = [entry for index, entry in enumerate(allowed) if index not in used]
        for entry in report['stale']:
            print(f'stale allowlist entry: {entry}', flush=True)
    write_report(args.output, report)
    print(f'Report: {args.output}')
    return int(failed)


if __name__ == '__main__':
    sys.exit(main())
