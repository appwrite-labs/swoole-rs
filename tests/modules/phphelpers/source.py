"""The pin and file hashes the SDK checks share."""
import hashlib
from pathlib import Path
import sys
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_upstream import COMMIT as PIN


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

