"""Check that the engines under test are a native build and the Cargo build's clients-enabled extension."""
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.ext_identity import attest as identity, sha  # noqa: F401  (the runners import sha from here)

CLIENTS = 1048576


def attest(manifest_path, identities):
    if len(identities) != 2 or {bool(item['rust_feature_bits'] & CLIENTS) for item in identities} != {False, True}:
        raise ValueError('Require native and selected clients engines')
    return identity(manifest_path, next(item for item in identities if item['rust_feature_bits'] & CLIENTS), CLIENTS)
