#!/usr/bin/env python3
"""The clients gates' shared helpers: a function body cut from a source file and a
command runner that records its evidence."""
import base64
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_upstream import COMMIT as PIN


LOG = None
SERIAL = 0


def run(args):
    global SERIAL
    command = list(map(str, args))
    # A program linking the Rust archive needs the system zlib and OpenSSL its libz-sys
    # (#583) and openssl-sys (#1541) bindings use, as the extension build already links them.
    if command[0] in ('c++', 'cc') and any(part.endswith('.a') for part in command):
        command += subprocess.check_output(['pkg-config', '--libs', 'zlib', 'openssl'], text=True).split()
    result = subprocess.run(command, capture_output=True)
    record = {"command": command, "returncode": result.returncode}
    for name in ("stdout", "stderr"):
        raw = getattr(result, name)
        try:
            record[name] = raw.decode("utf-8")
        except UnicodeDecodeError:
            record[name] = raw.decode("utf-8", errors="backslashreplace")
            record[name + "_raw_base64"] = base64.b64encode(raw).decode("ascii")
    if LOG is not None:
        SERIAL += 1
        (LOG / f"{SERIAL:03d}.json").write_text(json.dumps(record, indent=2) + "\n")
    result.check_returncode()
    # Keep the existing strict success contract; recording invalid bytes never
    # converts a failed decode/warned tool result into a passing gate.
    result.stderr.decode("utf-8")
    return result.stdout.decode("utf-8")


def body(text, signature):
    start = text.index(signature)
    opening = text.index('{', start)
    depth = 1
    pos = opening + 1
    # These actual bodies contain no brace characters within literals/comments.
    while depth:
        depth += (text[pos] == '{') - (text[pos] == '}')
        pos += 1
    return text[start:pos]
