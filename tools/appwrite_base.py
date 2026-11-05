#!/usr/bin/env python3
"""Rewrite appwrite/docker-base's Dockerfile so only its Swoole stage changes.

The new stage builds Swoole from this repository (build context `swoole-rs`) with
Appwrite's own configure flags, read from their stage: `upstream` is stock
Swoole at this repository's pin, the control; `hybrid` is the all-modules hybrid.
Everything else in the image is untouched, so the two images differ only in Swoole.
A build context without .git gets a throwaway repository, because the hybrid build
records the source revision.
"""
import argparse
from pathlib import Path
import re
import sys

STAGE = re.compile(r'^FROM compile AS swoole\n.*?strip /artifacts/swoole\.so\n', re.M | re.S)


def rewrite(dockerfile, variant):
    match = STAGE.search(dockerfile)
    if not match:
        raise ValueError('Swoole stage not found; appwrite/docker-base changed shape')
    flags = re.findall(r'^\s+(--(?:enable|with)-[\w-]+(?:=\S+)?)', match.group(0), re.M)
    configure = ' '.join(f'--configure={flag}' for flag in flags if flag != '--enable-sockets')
    build = 'python3 tools/build_upstream.py --no-ccache --php-sockets ' + configure
    if variant == 'hybrid':
        build += f' && \\\n  cargo fetch --locked && \\\n  python3 tools/build_hybrid.py --no-ccache --php-sockets {configure}'
        output = 'target/hybrid/rust-php-sockets/modules/swoole.so'
    else:
        output = 'target/upstream/src/modules/swoole.so'
    stage = f'''FROM compile AS swoole
COPY --from=swoole-rs / /swoole-rs
RUN \\
  apk add --no-cache python3 pcre2-dev pkgconf && \\
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none && \\
  export PATH="/root/.cargo/bin:$PATH" && \\
  cd /swoole-rs && \\
  {{ [ -d .git ] || {{ git init -q && git add -A && git -c user.name=build -c user.email=build@localhost commit -qm source; }}; }} && \\
  {build} && \\
  cp {output} /artifacts/swoole.so && \\
  strip /artifacts/swoole.so
'''
    return dockerfile[:match.start()] + stage + dockerfile[match.end():]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('dockerfile', type=Path, help="appwrite/docker-base's Dockerfile")
    parser.add_argument('--variant', choices=['hybrid', 'upstream'], required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.write_text(rewrite(args.dockerfile.read_text(), args.variant))
    return 0


if __name__ == '__main__':
    sys.exit(main())
