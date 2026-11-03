"""Compare error_handler_bailout.php's output and exit code on two engines.

    python3 tests/modules/zend/run.py <upstream swoole.so> <hybrid swoole.so> [php]
    python3 tools/dev.py docker -- tests/modules/zend/run.py target/upstream/src/modules/swoole.so \
        target/hybrid/dev-rust-.../modules/swoole.so
"""
import subprocess
import sys
from pathlib import Path

CASES = ['atomic_coercion', 'process_signal', 'table_column', 'table_conversion', 'timer_after',
         'event_del', 'cookie_name', 'http_client_file', 'server_coro_accept']
SCRIPT = Path(__file__).with_name('error_handler_bailout.php')


def run(php, extension, case, mode):
    result = subprocess.run([php, '-n', '-d', f'extension={extension}', str(SCRIPT), case, mode],
                            capture_output=True, text=True, timeout=30)
    return result.stdout + result.stderr, result.returncode


def main():
    upstream, hybrid = sys.argv[1], sys.argv[2]
    php = sys.argv[3] if len(sys.argv) > 3 else 'php'
    failed = 0
    for case in CASES:
        for mode in ['exit', 'fatal']:
            want, got = run(php, upstream, case, mode), run(php, hybrid, case, mode)
            if want == got:
                print(f'ok {case} {mode}: exit {got[1]}: {got[0].strip()!r}')
            else:
                failed += 1
                print(f'FAIL {case} {mode}\n--- upstream (exit {want[1]})\n{want[0]}--- hybrid (exit {got[1]})\n{got[0]}')
    sys.exit(1 if failed else 0)


if __name__ == '__main__':
    main()
