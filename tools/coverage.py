#!/usr/bin/env python3
"""Line coverage of crates/ in the Cargo swoole.so, from the profiles its runs wrote (#2254).

  coverage.py --engine SWOOLE_SO --profiles DIR --output DIR

.github/workflows/coverage.yml builds the extension with -Cinstrument-coverage and runs the
PHPT corpus, the corpus apps and swoole/library's suite with LLVM_PROFILE_FILE naming a %Nm
pool, so every process that loads it, forked workers too, merges its counters there when it
exits. A process that dies by a signal or _exit writes nothing, so code only such a process
ran reads as uncovered. native/ and the bundled C aren't instrumented (a second profile
runtime in the same library would clash with rustc's). This merges every .profraw under
--profiles and writes, into --output:

summary.json  per crate: lines, covered lines, and the entry points no run reached
              (zif_/zim_ handlers and swr_v1_ exports), with the commit measured
coverage.md   the same as tables, which the badge links to
lcov.info     llvm-cov's per-line export
html/         llvm-cov's annotated source

The per-crate table goes to stdout.
"""
import argparse
import json
import re
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
ENTRY_POINTS = ('zif_', 'zim_', 'swr_v1_')
# Paths llvm-cov reports beside crates/: std, crates.io and generated bindings.
IGNORE = r'/rustc/|/\.cargo/|/target/'


def llvm_tool(name):
    """The llvm-tools component's binary, which reads rustc's profile format."""
    sysroot = subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip()
    host = next(line.split()[1] for line in subprocess.check_output(['rustc', '-vV'], text=True).splitlines()
                if line.startswith('host:'))
    return str(Path(sysroot) / 'lib/rustlib' / host / 'bin' / name)


def parse_lcov(text, root=ROOT):
    """({crate: {'lines', 'covered'}}, {entry point: (crate, count)}) from lcov records of files
    under crates/ (absolute under root, or relative); an entry point recorded twice keeps its
    highest count."""
    crates, entries, crate = {}, {}, None
    for line in text.splitlines():
        key, _, value = line.partition(':')
        if key == 'SF':
            path = value.removeprefix(f'{root}/')
            crate = path.split('/')[1] if path.startswith('crates/') else None
            if crate:
                crates.setdefault(crate, {'lines': 0, 'covered': 0})
        elif crate is None:
            continue
        elif key in ('LF', 'LH'):
            crates[crate]['lines' if key == 'LF' else 'covered'] += int(value)
        elif key == 'FNDA':
            count, _, function = value.partition(',')
            if function.startswith(ENTRY_POINTS):
                entries[function] = (crate, max(int(count), entries.get(function, (crate, 0))[1]))
    return crates, entries


def handler_rows(root=ROOT):
    """{zif_/zim_ name: crate} from `name => body;` rows of a handlers! invocation. llvm-cov files a
    macro-made function under the macro's own file, crates/zend, not the crate that invoked it."""
    homes = {}
    for source in (root / 'crates').glob('*/src/**/*.rs'):
        for name in re.findall(r'\b((?:zif|zim)_\w+)\s*=>', source.read_text(errors='replace')):
            homes[name] = source.relative_to(root / 'crates').parts[0]
    return homes


def summarize(crates, entries, homes, commit):
    rows = {name: {**crate, 'uncovered_entry_points': []} for name, crate in sorted(crates.items())}
    for function, (crate, count) in sorted(entries.items()):
        if not count:
            rows.setdefault(homes.get(function, crate), {'lines': 0, 'covered': 0, 'uncovered_entry_points': []})[
                'uncovered_entry_points'].append(function)
    return {'commit': commit, 'lines': sum(r['lines'] for r in rows.values()),
            'covered': sum(r['covered'] for r in rows.values()), 'crates': rows}


def percent(row):
    return 100 * row['covered'] / row['lines'] if row['lines'] else 0.0


def table(summary):
    rows = ''.join(f"| {name} | {row['lines']:,} | {percent(row):.1f}% | {len(row['uncovered_entry_points'])} |\n"
                   for name, row in sorted(summary['crates'].items(), key=lambda item: percent(item[1])))
    return (f'| crate | lines | covered | entry points not reached |\n| --- | ---: | ---: | ---: |\n{rows}'
            f"| **crates/** | {summary['lines']:,} | {percent(summary):.1f}% | "
            f"{sum(len(r['uncovered_entry_points']) for r in summary['crates'].values())} |\n")


def markdown(summary):
    lists = ''.join(f'\n## {name}\n\n' + ''.join(f'- `{f}`\n' for f in row['uncovered_entry_points'])
                    for name, row in sorted(summary['crates'].items()) if row['uncovered_entry_points'])
    return (f"# Coverage of crates/\n\nLines of `crates/` the Cargo swoole.so ran under the PHPT corpus, the corpus "
            f"apps and swoole/library's suite, measured on `{summary['commit'][:7]}` (`tools/coverage.py`, "
            f'nightly `.github/workflows/coverage.yml`). A process that exits by a signal or `_exit` records '
            f'nothing.\n\n{table(summary)}\n# Entry points no run reached\n\n'
            f'`zif_`/`zim_` handlers and `swr_v1_` exports whose counter stayed zero, per crate: candidates for '
            f'targeted differential tests ([#2249](https://github.com/appwrite-labs/swoole-rs/issues/2249)).\n{lists}')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--engine', type=Path, required=True, help='The instrumented swoole.so the runs loaded')
    parser.add_argument('--profiles', type=Path, required=True, help='Searched for .profraw files')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    raw = sorted(args.profiles.rglob('*.profraw'))
    if not raw:
        raise SystemExit(f'coverage.py: no .profraw files under {args.profiles}')
    (inputs := args.output / 'profraw.txt').write_text(''.join(f'{path}\n' for path in raw))
    profile = args.output / 'merged.profdata'
    # One truncated pool file (a process killed mid-write) shouldn't lose the rest.
    subprocess.run([llvm_tool('llvm-profdata'), 'merge', '-sparse', '--failure-mode=all',
                    f'--input-files={inputs}', '-o', profile], check=True)
    cov = [llvm_tool('llvm-cov'), '-instr-profile', profile, f'-ignore-filename-regex={IGNORE}', args.engine]
    lcov = subprocess.run([cov[0], 'export', '-format=lcov', *cov[1:]], check=True, capture_output=True, text=True).stdout
    (args.output / 'lcov.info').write_text(lcov)
    subprocess.run([cov[0], 'show', '-format=html', f"-output-dir={args.output / 'html'}", *cov[1:]], check=True)
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    summary = summarize(*parse_lcov(lcov), handler_rows(), commit)
    (args.output / 'summary.json').write_text(json.dumps(summary, indent=1) + '\n')
    (args.output / 'coverage.md').write_text(markdown(summary))
    print(f'{len(raw)} profiles\n\n{table(summary)}')


if __name__ == '__main__':
    sys.exit(main())
