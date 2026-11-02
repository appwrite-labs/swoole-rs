# PHP classes in Rust

Goal #754 moves the Zend surface (`ext-src/`) into Rust. `Swoole\Atomic` was
the first family to move (#755); copy it.

## Crates

- `crates/zend-sys`: bindgen at build time against `php-config --includes`
  (`$PHP_CONFIG`; `build_hybrid.py` passes `--php-config`), so struct layouts
  follow that PHP's version and ZTS mode. bindgen's layout assertions fail the
  build on a mismatch. `src/shim.c` is the macros and inline functions bindgen
  can't see; add one there when a macro is version-dependent or touches an
  anonymous union (`create_object`).
- `crates/zend`: the safe layer. `Call` (argument count, `$this`, `long`/`double`
  parsed as `Z_PARAM_LONG`/`Z_PARAM_DOUBLE`, `set_long`/`set_bool`), `Object<T>`
  (`struct { T data; zend_object std; }`: `create`, `data`, `install`, `dtor`),
  `Class` (registration from a method table), `unset_property_deny`,
  `throw_exception`/`throw_error`. Add wrappers as callers need them.
- One `crates/php<family>` crate per family (`crates/phplocks`), owned by the
  module that owns its `ext-src` file. It calls the mechanism crate directly
  (Rust calls Rust).

All three sit behind a `php` feature and are empty without it, so the
workspace builds without PHP. The module's `crates/ffi` feature enables
`<crate>/php` and re-exports the crate. The PHP-facing code lives in its own
crate so the native gates that link `libswoole_rs_ffi.a` into executables never
pull in objects with Zend imports.

## The pattern

What stays C++ in the `ext-src` file (edited through `migration/patches/<module>/`):

- the `stubs/*_arginfo.h` include and upstream's `PHP_ME` tables, unchanged:
  arginfo and reflection stay byte-identical. The method declarations lose
  `static`; Rust exports the handlers under Zend's own names
  (`zim_<class>_<method>`), so the tables need no edit.
- MINIT: one call into Rust, passing the tables and anything only C++ can
  reach (here `sw_mem_pool()` alloc/free and `swoole_exception_ce`).
- C++ types other files still use (here `ThreadResource` subclasses, and the
  class-entry globals `swoole_thread.cc` tests against, which Rust fills).

What moves to Rust: class registration and flags, object handlers
(`create_object`, `free_obj`, clone and unset denial), object layout and every
method body. Declare the `swr_v1_*` entry points in the module's
`native/<module>/` header with forward-declared Zend structs; guard ZTS-only ones
with `#ifdef SW_THREAD`, which `config.h` defines, so the bridge's export
inventory only expects what the build links.

A method:

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
  loop (timers, the reactor, Server's `onReceive`/`onRequest`) keep a C++
  trampoline that does exactly this:

```cpp
static void timer_callback(Timer *timer, TimerNode *tnode) {
    if (swr_v1_..._callback(tnode->data)) {  // Rust: true if it trapped a bailout
        timer->abandon_selection();
        zend_bailout();
    }
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

`tests/modules/zend/error_handler_bailout.php` checks this per family against
upstream: an error handler that `exit()`s, and one that raises a fatal error,
on a Rust-raised warning.

## Checks

- `cargo clippy --workspace --all-targets --all-features` and `cargo test
  --workspace --all-features` (the `rust` job installs PHP headers and libclang).
- Syntax-check the patched C++ (see the brief in AGENTS.md's module rules), then
  the hybrid and the family's PHPTs on Linux:
  `python3 tools/dev.py docker -- tools/build_hybrid.py --module <module> --jobs 4`
  and `corpus/upstream/run.py --case ...` against both engines.
- A callgrind A/B of a tight method loop against upstream, in the PR.
- Alpine (musl) builds build scripts with `-C target-feature=-crt-static`
  (merge tier); a static build script cannot `dlopen` libclang.
