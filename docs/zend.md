# PHP classes in Rust

The Zend surface (upstream's `ext-src/`) is Rust. `Swoole\Atomic`
(`crates/phplocks`) is the smallest complete family to read.

## Crates

- `crates/zend-sys`: bindgen at build time against `php-config --includes`
  (`$PHP_CONFIG`; `build_ext.py` sets it from `--php-config`), so struct layouts
  follow that PHP's version and ZTS mode. bindgen's layout assertions fail the
  build on a mismatch. `src/shim*.c` are the macros and inline functions bindgen
  can't see that have to stay C ([What stays C](#what-stays-c)); anything else
  is Rust over the bindgen'd layouts (`zend::value::set`, `empty_string`,
  `module_loaded`, a `php_stream`'s bitfields).
- `crates/zend`: the safe layer. `Call` (argument count, `$this`, `long`/`double`
  parsed as `Z_PARAM_LONG`/`Z_PARAM_DOUBLE`, `set_long`/`set_bool`), `Object<T>`
  (`struct { T data; zend_object std; }`: `create`, `data`, `install`, `dtor`;
  [Objects](#objects) makes it safe),
  `Class` (registration from a method table), `unset_property_deny`,
  `throw_exception`/`throw_error`, and values: `&Value` (a borrowed zval, from
  `Call::value` or `Value::from_ptr`: `as_long`, `as_double`, `str`, `array`,
  `object`, `class`, `pointer` read a type without converting it, `long`,
  `double`, `string`, `truthy` convert as Zend does, `dereferenced` follows a
  reference), `&Array` (`get`, `find`, `entries`, `for_each`, `count`,
  `is_list`), `&ZStr` (a borrowed string: `bytes`, `c_str`, `parse_quantity`),
  `ZString` and `Owned`/`OwnedArray` (`push_*`, `insert_*`), released in `Drop`
  or handed to the return slot with `Call::set_owned`. `Call`'s parsers have
  borrowed forms too (`zstr`, `zstr_or_null`, `array_ref`, `bool_or_null`). A
  family crate reads values through these with no `unsafe` of its own
  (`crates/phpredis/src/format.rs`, `crates/phpswoole/src/options.rs`); a raw
  pointer becomes a `&Value`, `&Array` or `&ZStr` once, at the boundary. A
  family's MINIT is `minit!(name => body)`, whose `body(&Minit)` registers
  constants, classes and function aliases safely; a callable that a C owner
  such as a timer holds runs through `Callable::callbacks::<impl Holder>()`.
  `crates/phptimer` uses no `unsafe` at all. Add wrappers as callers need them.
- One `crates/php<family>` crate per family (`crates/phplocks`). It calls the
  mechanism crate directly (Rust calls Rust).

All three sit behind a `php` feature and are empty without it, so the
workspace builds without PHP. `crates/ffi`'s `php` feature turns on each
family crate's `php`. `crates/ext` builds `swoole.so` with `php` on (fat LTO under the benchmarks'
`--profile release-lto`); `tools/build_ext.py --native` also builds the Zend-free
`libswoole_rs_ffi.a` without it, which the native gates link into executables.

## Methods

```rust
handlers! {
    leaf:                                        // hot: no bailout check after the body
    zim_swoole_atomic_add => word::add::<Atomic>; // Zend name => body
}

fn fetch<H: Holder>(call: &Call, op: fn(&H::Word, i64) -> i64) {
    match call.arity(0, 1).and_then(|()| call.long(1, 1)) {
        Ok(value) => call.set_long(word::<H>(call).map_or(0, |word| op(word, value))),
        Err(_) => failed(call),                  // ZEND_PARSE_PARAMETERS_END_EX(RETURN_FALSE)
    }
}
```

Speed: a method crosses crates (`zend`, the mechanism crate) and rustc inlines
non-generic functions across crates only when they are `#[inline]`. Keep the
`zend` call-frame accessors `#[inline]`, and give the mechanism a plain
`#[inline] pub` Rust function beside its `swr_v1_*` export (as
`locks::Lock` sits beside `swr_v1_locks_lock`); calling the export
costs Atomic::add about 40 instructions.

## Objects

A class whose objects carry Rust state (Swoole's `SW_SET_CLASS_CUSTOM_OBJECT`)
implements `zend::Custom` for that state and registers with
`Minit::register_object::<T>`, which installs generated create and free
handlers: `Custom::new` makes each object's data, and the data drops with the
object. `Call::custom::<T>()` borrows `$this`'s data and `Value::custom::<T>()`
any value's, both `None` for an object of another class; `new_object(data)`
makes one without running its constructor. The data is shared while a method
runs PHP code, so mutable state sits in `Cell`s and `OnceCell`s.

```rust
static LOCK: CustomClass<Held> = CustomClass::new();
struct Held(OnceCell<Lock>);              // unset until __construct

impl Custom for Held {
    fn class() -> &'static CustomClass<Self> { &LOCK }
    fn new() -> Self { Self(OnceCell::new()) }
}
```

A `CustomClass<T>` serves one data type, so a handler can't read another
class's objects as `T`. A type that sets `Custom::DESTRUCTS` also gets a
`dtor_obj` handler, which runs `__destruct` and then `Custom::destruct` (the
channel frees its queued values there). `crates/phplocks` and
`crates/phpcollections` use no `unsafe` at all.

Mirror upstream's ZPP block exactly: the same min/max, defaults and failure
return value. Errors are Zend's own (`zend_wrong_parameters_count_error`,
`zend_wrong_parameter_error`), so messages match.

## Arginfo and method tables

`crates/phpstubs` is every upstream stub's arginfo as Rust statics, one module
per `ext-src/stubs/*.stub.php`, named as gen_stub names them
(`phpstubs::atomic::arginfo_class_Swoole_Atomic_add`). `tools/arginfo.py`
generates it from the pin's stubs; `--check` (the PR tier's `ext` job) fails
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
  It (and `call_method`) calls nothing once a bailout is pending: upstream
  never ran a callback past the first bailout.
- `zend::bailout::trap(body, arg)` does the same for any `body`: C or C++, or
  a Rust `unsafe extern "C" fn` whose frame owns nothing that needs a drop, so
  the bailout's jump to the shim's `setjmp` skips no destructor (crates/phpserver
  runs `Server::start` and `addProcess`'s worker this way).
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
- `Props` writes (`zend_update_property`: a subclass's `__set` or property
  hook runs PHP), skipped once a bailout is pending; and the release of an
  object, array or reference (`Owned`'s drop, `Call::assign`, `ArrayMut::update`:
  `__destruct`, or the cycle collector a possible root starts).

Each returns (or, for the converters, defers) a `Bailout`. The one exception
is an `E_ERROR` its caller relies on not returning (crates/phpserver's
"Invalid instance of"): it calls `php_error_docref` directly, from frames that
own nothing. The usual owner is
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
relays the outcome after dropping its owners. Only `Swoole\Thread`, which needs
ZTS, raises a whole-thread exit, so only ZTS builds keep a record per scope;
on NTS a scope is a plain `zend_try`. Callbacks invoked below Rust must
catch C++ exceptions inside the native body, return a typed fault, and rethrow
only after Rust returns.

## What stays C

The C compiled from `crates/` into `swoole.so` (outside `crates/oracle`) is
what Rust can't express, or can't express without more `unsafe`. Each file's
header names its reasons from this list:

- **bailout**: the call can run PHP code (a destructor, `__get`, an
  autoloader, an error handler) that bails out, and `zend_try`'s `setjmp` must
  sit in a C frame ([Calling PHP](#calling-php-bailouts)).
- **globals**: it reads or writes `EG`, `CG`, `PG`, `SG`, `BG`, `FG`, `OG` or
  `SIGG`. Under ZTS these are TSRM slots reached through a thread-local cache
  and offsets only the macros know.
- **allocator**: it allocates or frees through Zend's allocator (`emalloc`,
  `pemalloc`, `zend_string_*`, `array_init`, `OBJ_RELEASE`, `smart_str`), whose
  entry points take the caller's file and line in debug builds
  (`ZEND_FILE_LINE_DC`).
- **version**: the macro, field or signature differs between the PHP versions
  CI builds (`PHP_VERSION_ID`, a feature `#ifdef`), or the member sits in an
  anonymous union whose bindgen name changes (`ce->create_object`,
  `ce->parent`); the same holds for library versions (`ARES_VERSION`).
- **variadic**: a C variadic or `va_list` function, which stable Rust can't
  define (`crates/corebase/src/format.c`, `crates/ffi/src/fatal.c`,
  `shim_errors.c`, `shim_host.c`).
- **private**: the layout is private to a source file or library release:
  `zend_closure` behind `ZEND_CLOSURE_OBJECT`, `zend_execute.c`'s static helpers
  (`crates/phpstdext/src/stdext.c`), `pdo_dbh_t`, ext/sockets' `php_socket`,
  liburing's ring (`crates/uring-sys/src/shim.c`).
- **budget**: a `static inline` over public fields (`Z_TRY_ADDREF_P`,
  `ZVAL_COPY`, `ZVAL_STR`, `GC_ADDREF`, `Z_OBJPROP_P`, `ZEND_HASH_FOREACH`)
  whose callers hold raw pointers: a Rust port adds `unsafe` and removes none,
  which `tools/budget.py` refuses. One moves once its callers hold a `Value`
  or `Owned`, where it can be a safe method.

A shim that fits none of these is a port waiting to happen: write it in
`crates/zend` against the bindgen'd layout, as `zend::module_loaded` replaced
`swr_zend_module_loaded`.

## Stdext

The `stdext` feature of `crates/ffi` (`phpstdext/active`) tracks native
`SW_STDEXT`; `tools/build_ext.py --stdext` turns it on.
Feature-enabled Rust checks use `--features php,active`.

## Checks

- `cargo clippy --workspace --all-targets --all-features` and `cargo test
  --workspace --all-features` (the `rust` job installs PHP headers and libclang).
- The Cargo build and the family's PHPTs on Linux:
  `python3 tools/dev.py docker -- tools/build_ext.py`
  and `corpus/upstream/run.py --case ...` against both engines.
- A callgrind A/B of a tight method loop against upstream, in the PR.
- Alpine (musl) builds build scripts with `-C target-feature=-crt-static`
  (merge tier); a static build script cannot `dlopen` libclang.
