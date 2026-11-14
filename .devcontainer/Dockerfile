# The same OS and packages as the CI runners (ubuntu-24.04, PHP 8.3).
FROM ubuntu:24.04
COPY install.sh /tmp/install.sh
RUN /tmp/install.sh && rm -rf /var/lib/apt/lists/* /tmp/install.sh
# GitHub CLI: the executor loop drives GitHub through `gh`, and unlike the CI
# runners (which ship it) a bare container has none. install.sh stays the package
# list shared with CI, so gh — which CI already has — is added here instead, with
# valgrind, which only the callgrind job installs (benchmarks/callgrind.py), and clang
# and llvm, which only the sanitizer run installs (tools/asan.py).
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
        -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && chmod go+r /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
        > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update -q && apt-get install -yq --no-install-recommends gh valgrind clang libclang-rt-dev llvm \
    && rm -rf /var/lib/apt/lists/*
# cbindgen for tools/headers.sh, which the pre-commit hook runs: the release binary of
# the version pr.yml installs (#3332), checked against its hash.
RUN case "$(dpkg --print-architecture)" in \
        amd64) asset=cbindgen-ubuntu22.04 sum=27ac237e2ad3ffaffc8ecce0c03e3caa1d1b09e691906c3cb0967367ff6d783d ;; \
        arm64) asset=cbindgen-ubuntu22.04-aarch64 sum=1838dfc7ddfb7e941556505fa6eebc5c28553eb03118980262c760594e240bdb ;; \
    esac \
    && curl -fsSL --retry 5 -o /usr/local/bin/cbindgen "https://github.com/mozilla/cbindgen/releases/download/0.29.4/$asset" \
    && echo "$sum  /usr/local/bin/cbindgen" | sha256sum -c - \
    && chmod +x /usr/local/bin/cbindgen
# rustup installs the toolchain pinned in rust-toolchain.toml on first use.
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none \
    && chmod -R a+w /usr/local/rustup /usr/local/cargo
