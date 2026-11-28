# Pinned third-party Hyperf application

This corpus runs the unchanged
[Hyperf application skeleton](https://github.com/hyperf/hyperf-skeleton) at commit
`855dd7ef145d9e1e3d0500d958ac55d51cbf6ef0`. The source archive and dependency lock
SHA-256 values are recorded in `manifest.json`. It is a real framework skeleton,
not a production business application.

Prepare explicitly, once; this is the only step that accesses the network:

```sh
python3 corpus/thirdparty/hyperf.py --prepare
```

This clones into `target/thirdparty/hyperf`, checks out the exact commit, copies
the checked-in `composer.lock`, and runs Composer install with **no dev packages,
no scripts and no plugins**. `--composer` chooses a Composer executable. Normal
test runs never fetch source or resolve/install dependencies; missing inputs fail
with an actionable error. Runtime dependencies are pinned to 82 packages. The
lock was resolved from the unchanged application's `composer.json`, using the
five temporary Symfony 7 constraints recorded in the manifest to avoid host PHP
8.5 selecting PHP-8.4-only runtime packages. Do not use `composer update` during
preparation. Development-only lock entries are not installed.

Run upstream and the Cargo build sequentially:

```sh
python3 corpus/thirdparty/hyperf.py \
  --engine upstream=/absolute/path/to/upstream/swoole.so \
  --engine cargo=target/ext/swoole.so \
  --output target/reports/thirdparty-hyperf.json
```

`--php` selects PHP; repeat `--php-extension` for required extra modules, including
PDO/SQLite and tokenizer where these are not compiled into PHP. Requirements include Swoole's
HTTP server, coroutines, coroutine HTTP client, PDO SQLite and the framework's
PHP extensions from the lock. Swoole short names are explicitly disabled as
required by Hyperf.

## What executes

Every engine gets a disposable tree extracted from the pinned Git archive, the
checked lock and copied prepared vendor directory. The tracked source must be
clean and at the expected commit. The driver verifies the archive/lock hashes,
installed package versions/source references against the lock, and every tracked
file's unchanged bytes after the run. It permits generated runtime/vendor files;
no controller, route, server option, container definition or app dependency is
edited. Dependency contents rely on Composer's installation; the installed
metadata comparison is not a full vendor-content attestation.

1. Start the normal `bin/hyperf.php start` entry point. The unchanged server binds
   `0.0.0.0:9501` and uses `swoole_cpu_num()` workers.
   The driver refuses to start if that port is already occupied. Run this corpus
   serially on an isolated test host/container because the application fixes its
   listening port and worker count.
2. Check six actual application requests: default GET, UTF-8/query decoding,
   JSON POST, form POST, a 404 and the empty favicon route. Expected GET/POST
   JSON is compared exactly. A final request checks surviving server health.
3. In an **external probe**, initialize the unchanged application's actual
   classloader/container. Use its Hyperf Guzzle `ClientFactory` inside a Hyperf
   coroutine with hook flags zero, selecting the framework's `CoroutineHandler`
   for a local HTTP request. Verify its exact response.
4. Use Hyperf's existing `Cache\\Driver\\SqliteDriver`, with explicit external
   probe configuration pointing to a disposable SQLite file, to persist a UTF-8
   value containing a NUL byte. Verify the value through that driver and the
   database row count. A second independent PHP process reads it, then a third
   reads it after the whole HTTP server exits. This exercises framework cache,
   pooling, serialization, coordinator teardown and disk persistence.
5. Send SIGTERM to the isolated master, require normal exit 0 and listener
   closure. Always clean up the owned process group on failure or timeout. Never
   run broad Swoole cleanup scripts. The external cache probe resumes Hyperf's
   worker-exit coordinator so its housekeeping coroutine can end normally.

The external probe is explicitly harness code, not an unchanged application
endpoint. This skeleton has no persistence or outbound-I/O route. These checks
therefore **do not** claim end-to-end controller-to-database coverage, MySQL/
Redis integration, SQLite coroutine hook coverage, production load, all-worker
health, or general resource-leak detection. Hook flags zero still exercise
Hyperf's coroutine HTTP handler; PDO SQLite operations are synchronous.
