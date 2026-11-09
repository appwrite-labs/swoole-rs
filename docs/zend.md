# PHP classes in Rust

The Zend surface (upstream's `ext-src/`) is Rust. `Swoole\Atomic`
(`crates/phplocks`) is the smallest complete family to read.

## Crates

- `crates/zend-sys`: bindgen at build time against `php-config --includes`
  (`$PHP_CONFIG`; `build_hybrid.py` passes `--php-config`), so struct layouts
  follow that PHP's version and ZTS mode. bindgen's layout assertions fail the
  build on a mismatch. `src/shim*.c` are the macros and inline functions bindgen
  can't see; add one there when a macro is version-dependent or touches an
  anonymous union (`create_object`).
- `crates/zend`: the safe layer. `Call` (argument count, `$this`, `long`/`double`
  parsed as `Z_PARAM_LONG`/`Z_PARAM_DOUBLE`, `set_long`/`set_bool`), `Object<T>`
  (`struct { T data; zend_object std; }`: `create`, `data`, `install`, `dtor`),
  `Class` (registration from a method table), `unset_property_deny`,
  `throw_exception`/`throw_error`. Add wrappers as callers need them.
- One `crates/php<family>` crate per family (`crates/phplocks`). It calls the
  mechanism crate directly (Rust calls Rust).

All three sit behind a `php` feature and are empty without it, so the
workspace builds without PHP. `crates/ffi`'s `php` feature turns on each
family crate's `php`. The hybrid build makes two archives: `libswoole_rs_php.a`
with `php` for the extension (thin LTO outside `--dev`), and the Zend-free
`libswoole_rs_ffi.a` without it, which the native gates link into executables.

## Methods

```rust
methods! {
    this_word;                                   // locate $this's state
    zim_swoole_atomic_add: u32 => word::add;     // Zend name => body
}

pub(crate) fn add<W: Word>(call: &Call, word: *mut W) {
    match call.arity(0, 1).and_then(|()| call.long(1, 1)) {
        Ok(value) => call.set_long(unsafe { W::apply(word, ADD_FETCH, value, 0) }),
        Err(_) => call.set_bool(false),        // ZEND_PARSE_PARAMETERS_END_EX(RETURN_FALSE)
    }
}
```

Speed: a method crosses crates (`zend`, the mechanism crate) and rustc inlines
non-generic functions across crates only when they are `#[inline]`. Keep the
`zend` call-frame accessors `#[inline]`, and give the mechanism a plain
`#[inline] pub` Rust function beside its `swr_v1_*` export (as
`locks::atomic32` sits beside `swr_v1_locks_atomic32`); calling the export
costs Atomic::add about 40 instructions.

Mirror upstream's ZPP block exactly: the same min/max, defaults and failure
return value. Errors are Zend's own (`zend_wrong_parameters_count_error`,
`zend_wrong_parameter_error`), so messages match.

## Arginfo and method tables

`crates/phpstubs` is every upstream stub's arginfo as Rust statics, one module
per `ext-src/stubs/*.stub.php`, named as gen_stub names them
(`phpstubs::atomic::arginfo_class_Swoole_Atomic_add`). `tools/arginfo.py`
generates it from the pin's stubs; `--check` (the PR tier's `hybrid` job) fails
if it is stale or if the same model, rendered as gen_stub's C, differs by a
byte from the pin's `*_arginfo.h`. The builders in `zend::arginfo` are one
`const fn` per Zend macro over the headers' own type bits, so the arrays are
the macros' on every PHP version.

A family's table is upstream's `PHP_ME` rows, in upstream's order (reflection
lists methods in table order, which is not always the stub's):

```rust
method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_lock___construct, &lock::arginfo_class_Swoole_Lock___construct, ZEND_ACC_PUBLIC;
        c"lock" => zim_swoole_lock_lock, &lock::arginfo_class_Swoole_Lock_lock, ZEND_ACC_PUBLIC;
    ];
}
```

## Calling PHP: bailouts

`exit()` and fatal errors leave PHP code by `zend_bailout()`, a `longjmp` to the
nearest `zend_try`. A `longjmp` must never cross a Rust frame, so Rust never
calls PHP code directly where it could bail out:

- `zend::bailout::call(fci, fcc)` runs `zend_call_function` inside a
  `zend_try` that lives in the C shim, and returns `Err(Bailout)` if it bailed.
- `zend::bailout::trap(body, arg)` does the same for a C or C++ `body` (for
  example Swoole's coroutine-aware `zend::function::call`, lent by C++).
- `Bailout` is `#[must_use]`: finish the Rust work (release what you hold),
  return it to the C/C++ caller as a flag, and let that caller call
  `zend_bailout()` once the Rust frames are gone. Callbacks driven by a C++
  loop keep a C++ trampoline that does exactly this. A Rust fn the C++ loop
  calls directly (the Server's on* slots) does the same with `bailout::raise`
  as its last step, from a frame that owns nothing that needs a drop:

```rust
// crates/phpevents: a task the reactor's defer queue runs.
unsafe extern "C-unwind" fn deferred_free_object(object: *mut c_void) {
    bailout::raise(unsafe { free_object(object.cast()) }); // Err if it trapped a bailout
}
```

`zend::bailout::parse_func` is `Z_PARAM_FUNC` (Zend's own callback errors).

### Errors raised from Rust

A warning or notice runs the user error handler (`set_error_handler`), which
is PHP code: a fatal error in it, or `exit()` with no PHP frame below (a
reactor or timer callback), bails out. So every error Rust raises goes through
`shim_errors.c`, which runs it inside `zend_try`:

- `zend::php_error(level, message)`: `php_error_docref`.
- `zend::bailout::report*(reporter, ...)`: a C++ reporter (Swoole's
  `php_swoole_error`/`_fatal_error`/`_sys_error` live in C++), one function per
  argument shape.
- `Call`'s ZPP slow paths (null-to-scalar and float-to-int deprecations) and
  `zend::value::get_long`/`get_double`/`get_string` (`zval_get_*`: an object or
  array warns, `__toString` runs PHP).

Each returns (or, for the converters, defers) a `Bailout`. The usual owner is
`bailout::defer`: the bailout waits in a thread-local, nothing more is
reported, and the handler's last step, `bailout::finish()`, raises it
(`zend::handlers!` does this; so must every export C/C++ calls that can raise
an error, or it returns the bailout as a flag to its C++ trampoline). Never
call `php_error_docref`, `zend_error` or a C++ reporter directly from Rust.

`tests/modules/zend/run.py` (`error_handler_bailout.php`) checks this per family against
upstream: an error handler that `exit()`s, and one that raises a fatal error,
on a Rust-raised warning.

### Native scopes and C++ exceptions

`native/corebase/swoole_rs_zend_guard.h` defines native scope outcomes: normal
return (0), ordinary Zend bailout (1) and pending whole-thread exit (2). Rust
relays the outcome after dropping its owners. Callbacks invoked below Rust must
catch C++ exceptions inside the native body, return a typed fault, and rethrow
only after Rust returns. `native/corebase/swoole_rs_native_effect.h` is the
invoke/resume provider that MINIT registers once.

## Stdext

The `stdext` feature of `crates/ffi` (`phpstdext/active`) tracks native
`SW_STDEXT`; `tools/build_hybrid.py` derives it from the configure options.
Feature-enabled Rust checks use `--features php,active`.

## Checks

- `cargo clippy --workspace --all-targets --all-features` and `cargo test
  --workspace --all-features` (the `rust` job installs PHP headers and libclang).
- The hybrid and the family's PHPTs on Linux:
  `python3 tools/dev.py docker -- tools/build_hybrid.py --jobs 4`
  and `corpus/upstream/run.py --case ...` against both engines.
- A callgrind A/B of a tight method loop against upstream, in the PR.
- Alpine (musl) builds build scripts with `-C target-feature=-crt-static`
  (merge tier); a static build script cannot `dlopen` libclang.
