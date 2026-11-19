"""Run the fresh-socket bailout regression inside PHP's executor."""
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]


def main(php='php'):
    # Export only the fixture entry point. PHP supplies the Zend symbols at load time.
    build = subprocess.run(
        ['cargo', 'rustc', '--locked', '-p', 'swoole-rs-phpruntime', '--features', 'php',
         '--example', 'fresh_socket', '--message-format=json', '--',
         '-C', 'link-arg=-Wl,--exclude-libs,ALL'],
        cwd=ROOT, stdout=subprocess.PIPE, text=True, check=True,
    )
    artifact = next(
        row['filenames'][0] for row in map(json.loads, build.stdout.splitlines())
        if row['reason'] == 'compiler-artifact' and row['target']['name'] == 'fresh_socket'
    )
    code = """
        $probe = FFI::cdef('int fresh_socket_drop_count(void);', $argv[1]);
        $count = $probe->fresh_socket_drop_count();
        if ($count !== 1) {
            fwrite(STDERR, "expected one close hook after bailout, got $count\\n");
            exit(1);
        }
    """
    subprocess.run([php, '-d', 'ffi.enable=1', '-r', code, '--', artifact], check=True)


if __name__ == '__main__':
    main(*sys.argv[1:])
