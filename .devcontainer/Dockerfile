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
# rustup installs the toolchain pinned in rust-toolchain.toml on first use.
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none \
    && chmod -R a+w /usr/local/rustup /usr/local/cargo
