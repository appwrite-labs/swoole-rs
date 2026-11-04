"""Check that the engines under test are a native build and the build manifest's clients-enabled build."""
import hashlib
import json
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_upstream import COMMIT

def sha(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()
def attest(manifest_path,identities):
    if len(identities)!=2 or {bool(item['rust_feature_bits']&1048576) for item in identities}!={False,True}:
        raise ValueError('Require native and selected clients engines')
    manifest=json.loads(manifest_path.read_text())
    selected=next(item for item in identities if item['rust_feature_bits']&1048576)
    if selected['sha256']!=manifest['extension_sha256']:raise ValueError('Selected extension does not match manifest')
    if manifest['inputs']['upstream_commit']!=COMMIT:raise ValueError('Unexpected upstream')
    return dict(manifest=str(manifest_path),extension_sha256=selected['sha256'])
