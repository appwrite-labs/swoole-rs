"""A changed hook binding must fail the installed-header check."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]


class HookConstants(unittest.TestCase):
    def test_changed_curl_binding_fails(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            script = root / "tools/check_hook_constants.py"
            script.parent.mkdir()
            shutil.copyfile(ROOT / "tools/check_hook_constants.py", script)
            bindings = root / "crates/curl-sys/src/lib.rs"
            bindings.parent.mkdir(parents=True)
            bindings.write_text("pub const CURLOPT_URL: i32 = -1;\n")
            result = subprocess.run([sys.executable, script], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("static assertion", result.stderr)
            self.assertIn("CURLOPT_URL", result.stderr)


if __name__ == "__main__":
    unittest.main()
