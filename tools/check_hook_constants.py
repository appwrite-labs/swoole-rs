#!/usr/bin/env python3
"""Compile libcurl and libssh2 hook constants against the installed development headers."""
from pathlib import Path
import os
import re
import shlex
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def check(library, header, config):
    bindings = ROOT / "crates" / f"{library}-sys" / "src" / "lib.rs"
    generated = subprocess.check_output(["cbindgen", "--quiet", "--config", config, bindings], text=True)
    names = re.findall(r"^pub const (\w+)\s*:", bindings.read_text(), re.MULTILINE)
    if not names:
        raise ValueError(f"no constants found in {bindings}")
    source = [f"#include <{header}>", generated]
    for name in names:
        assertion = f'_Static_assert(RUST_{name} == {name}, "{name}");'
        # The hook supports libcurl 7.56 and gates this option on 7.67.
        if name == "CURLMOPT_MAX_CONCURRENT_STREAMS":
            assertion = f"#if LIBCURL_VERSION_NUM >= 0x074300\n{assertion}\n#endif"
        source.append(assertion)
    flags = subprocess.check_output(["pkg-config", "--cflags", f"lib{library}"], text=True)
    subprocess.run([*shlex.split(os.environ.get("CC", "cc")), *shlex.split(flags),
                    "-std=c11", "-fsyntax-only", "-x", "c", "-"],
                   input="\n".join(source) + "\n", text=True, check=True)
    print(f"Checked {library} bindings against {header}")


if __name__ == "__main__":
    with tempfile.NamedTemporaryFile(mode="w", suffix=".toml") as config:
        config.write('language = "C"\n[export]\nitem_types = ["constants"]\nprefix = "RUST_"\n')
        config.flush()
        check("curl", "curl/curl.h", config.name)
        check("ssh2", "libssh2.h", config.name)
