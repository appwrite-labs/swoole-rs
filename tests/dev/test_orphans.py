"""Every file under tests/modules/<module>/ is referenced by some other tracked file:
anywhere by its repo path, or from inside the same module directory by name, by its
stem as a quoted string or an import. A name match from another module does not count:
same-named gates are common. A file beside a same-stem .py (the C++ a gate compiles
with Path(__file__).with_suffix) is referenced when it is. Markdown does not count, so
a file only a README mentions is an orphan: wire it into a gate or delete it."""
from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
NAMES = re.compile(r'[\w.-]+')
QUOTED = re.compile(r'''['"]([\w.-]+)['"]|(?:import|from)\s+(?:[\w]+\.)*(\w+)''')
PATHS = re.compile(r'tests/modules/[\w./-]*\w')


def module(path):
    return '/'.join(path.split('/')[:3])


def orphans():
    files = subprocess.check_output(['git', '-C', str(ROOT), 'ls-files'], text=True).splitlines()
    seen, by_path = {}, set()
    for path in files:
        if path.endswith('.md') or not (ROOT / path).is_file():
            continue
        text = (ROOT / path).read_text(errors='ignore')
        by_path.update(p for p in PATHS.findall(text) if p != path)
        if path.startswith('tests/modules/'):
            seen[path] = set(NAMES.findall(text)), {a or b for a, b in QUOTED.findall(text)}
    def orphan(path):
        if path in by_path:
            return False
        name, stem = Path(path).name, Path(path).stem
        if any(other != path and module(other) == module(path) and (name in names or stem in stems)
               for other, (names, stems) in seen.items()):
            return False
        script = str(Path(path).with_suffix('.py'))
        return script == path or script not in seen or orphan(script)
    # pytest loads conftest.py files implicitly; nothing names them.
    return [path for path in files if path.startswith('tests/modules/') and not path.endswith('.md')
            and Path(path).name != 'conftest.py' and orphan(path)]


class Orphans(unittest.TestCase):
    def test_every_module_test_is_run(self):
        self.assertEqual(orphans(), [], 'wire these into a gate or delete them')


if __name__ == '__main__':
    unittest.main()
