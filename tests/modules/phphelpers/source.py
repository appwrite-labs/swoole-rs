"""Pinned common-helper source extraction used only by async/SDK qualification."""
import hashlib
from pathlib import Path
import sys
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_upstream import COMMIT as PIN


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def body(text, signature):
    start = text.index(signature)
    end = text.index('{', start) + 1
    depth = 1
    while depth:
        depth += (text[end] == '{') - (text[end] == '}')
        end += 1
    return text[start:end]
