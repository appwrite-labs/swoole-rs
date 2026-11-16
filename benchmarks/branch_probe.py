#!/usr/bin/env python3
"""Can this machine record the branch samples sample-based PGO needs (#4465, #4380)?

Reports the host's PMU exposure, then builds a small Rust program with line tables, records
it with `perf record -b`, and counts the branch-stack entries `perf script` decodes. With
--convert it turns that recording into an LLVM sample profile (llvm-profgen, else
create_llvm_prof), checks llvm-profdata finds samples in the program's hot function, and
rebuilds the program with `-Cprofile-sample-use`. Exits 0 only when every step it ran
produced real data; the report names the step that didn't and why.
"""
import argparse
import glob
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

PROGRAM = '''
#[inline(never)]
fn hot(x: u64) -> u64 {
    if x % 3 == 0 { x / 3 } else if x & 1 == 0 { x >> 1 } else { 3 * x + 1 }
}

fn main() {
    let mut steps = 0u64;
    for start in 1..std::hint::black_box(1_000_000u64) {
        let mut x = start;
        while x > 1 && x < 1 << 40 {
            x = hot(x);
            steps += 1;
        }
    }
    println!("{steps}");
}
'''
FLAGS = ['-O', '-Cdebuginfo=line-tables-only']
# A branch-stack entry, `from/to/flags...`, as `perf script -F brstack` prints it.
ENTRY = re.compile(r'0x[0-9a-f]+/0x[0-9a-f]+/')
# A function's line in `llvm-profdata show --sample`: `Function: NAME: TOTAL, HEAD, ...`.
FUNCTION = re.compile(r'^Function: (\S+): (\d+),', re.M)


def read(path):
    try:
        return Path(path).read_text().strip()
    except OSError:
        return None


def host():
    """Kernel, CPU, the cpuinfo flags that announce branch recording, the PMUs perf can open
    (with the branch-stack depth each reports), perf_event_paranoid and perf's version."""
    cpuinfo = read('/proc/cpuinfo') or ''
    model = re.search(r'^model name\s*:\s*(.*)$', cpuinfo, re.M)
    flags = re.search(r'^flags\s*:\s*(.*)$', cpuinfo, re.M)
    flags = set(flags.group(1).split()) if flags else set()
    pmus = sorted(os.listdir('/sys/bus/event_source/devices')) if os.path.isdir('/sys/bus/event_source/devices') else []
    perf = run(['perf', '--version'])
    return {'kernel': os.uname().release, 'machine': os.uname().machine,
            'cpu': model.group(1) if model else None, 'platform': read('/sys/class/dmi/id/product_name'),
            'flags': sorted(flags & {'arch_lbr', 'amd_lbr_v2', 'brs', 'hypervisor'}), 'pmus': pmus,
            'branches': {pmu: read(f'/sys/bus/event_source/devices/{pmu}/caps/branches') for pmu in pmus
                         if read(f'/sys/bus/event_source/devices/{pmu}/caps/branches')},
            'perf_event_paranoid': read('/proc/sys/kernel/perf_event_paranoid'),
            'perf': perf.stdout.strip() if perf.returncode == 0 else None}


def run(command, **kwargs):
    try:
        return subprocess.run(command, capture_output=True, text=True, timeout=300, **kwargs)
    except (OSError, subprocess.TimeoutExpired) as error:
        return subprocess.CompletedProcess(command, 127, '', str(error))


def tool(name):
    """NAME on PATH, else the newest LLVM's (Ubuntu keeps llvm-profgen off PATH)."""
    return shutil.which(name) or next(iter(sorted(glob.glob(f'/usr/lib/llvm-*/bin/{name}'), reverse=True)), None)


def branch_entries(script):
    """Samples carrying a branch stack, and the entries in them, in `perf script -F brstack` output."""
    stacks = [len(ENTRY.findall(line)) for line in script.splitlines()]
    return sum(1 for count in stacks if count), sum(stacks)


def function_samples(show, name):
    """Total samples of the functions whose (mangled) name contains NAME."""
    return sum(int(total) for function, total in FUNCTION.findall(show) if name in function)


def probe(directory, event='branches:u', convert=False):
    """The report: host facts, then each step's result, stopping at the first that fails."""
    report, directory = {'host': host(), 'verified': False}, Path(directory)
    source, binary, data = directory / 'main.rs', directory / 'main', directory / 'perf.data'
    source.write_text(PROGRAM)

    def step(name, result, ok):
        report[name] = {'command': ' '.join(map(str, result.args)), 'returncode': result.returncode}
        if not ok:
            report[name]['error'] = (result.stderr or result.stdout).strip()[-2000:]
            report['failed'] = name
        return ok

    built = run(['rustc', *FLAGS, source, '-o', binary])
    if not step('build', built, built.returncode == 0):
        return report
    recorded = run(['perf', 'record', '-b', '-e', event, '-c', '100003', '-o', data, '--', binary])
    if not step('record', recorded, recorded.returncode == 0):
        return report
    decoded = run(['perf', 'script', '-F', 'brstack', '-i', data])
    report['samples'], report['entries'] = branch_entries(decoded.stdout)
    if not step('decode', decoded, decoded.returncode == 0 and report['entries'] > 0):
        report['decode'].setdefault('error', 'no branch-stack entries in the recording')
        return report
    if convert:
        profile = directory / 'main.prof'
        if profgen := tool('llvm-profgen'):
            converted = run([profgen, f'--binary={binary}', f'--perfdata={data}', f'--output={profile}'])
        else:
            converted = run(['create_llvm_prof', f'--binary={binary}', f'--profile={data}', f'--out={profile}'])
        if not step('convert', converted, converted.returncode == 0 and profile.exists()):
            return report
        shown = run([tool('llvm-profdata') or 'llvm-profdata', 'show', '--sample', profile])
        report['hot_samples'] = function_samples(shown.stdout, '3hot')
        if not step('show', shown, shown.returncode == 0 and report['hot_samples'] > 0):
            report['show'].setdefault('error', 'no samples in the hot function')
            return report
        rebuilt = run(['rustc', *FLAGS, f'-Cprofile-sample-use={profile}', source, '-o', directory / 'main-pgo'])
        if not step('rebuild', rebuilt, rebuilt.returncode == 0):
            return report
    report['verified'] = True
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--event', default='branches:u',
                        help='The sampled event; on Intel BR_INST_RETIRED.NEAR_TAKEN:u is the AutoFDO default')
    parser.add_argument('--convert', action='store_true', help='Also convert, inspect and use the profile')
    parser.add_argument('--keep', type=Path, help='Keep the program, recording and profile in this directory')
    args = parser.parse_args()
    if args.keep:
        args.keep.mkdir(parents=True, exist_ok=True)
        report = probe(args.keep, args.event, args.convert)
    else:
        with tempfile.TemporaryDirectory() as directory:
            report = probe(directory, args.event, args.convert)
    print(json.dumps(report, indent=2))
    return 0 if report['verified'] else 1


if __name__ == '__main__':
    sys.exit(main())
