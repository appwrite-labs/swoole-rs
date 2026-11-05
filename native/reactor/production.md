# Loaded-PHP consumer profiles

`tests/modules/reactor/production_supported.py` compiles a PHP addon against a
Native and a selected production build and runs the same assertions under both
engines. Arguments:

```
--source PINNED --source-ref SHA --engine NAME=DSO (repeated)
--build-manifest MANIFEST --archive ARCHIVE
--selected-export export.json --native-export export.json --output UNIQUE
[--php php] [--php-config php-config] [--php-extension EXT]
```

It borrows the immutable export DSOs, checks their members are unchanged
before and after, and refuses a selected source that lacks the Rust files a
profile needs. Each profile also checks that the addon's imported methods
resolve to the expected engine and that the selected Rust policy symbols
resolve to the selected DSO.

## Profiles

| Flag | Exercises | Inventory |
| --- | --- | --- |
| (default) | readiness/timeout/ONCE over private AF_UNIX pairs | 14 cases / 210 assertions |
| `--callbacks` | constructor, set/replace/erase, end/exit, insertion/removal during traversal | 22 assertions |
| `--builtins` | defer/begin/end and installed deferred/idle/try-exit functions | 31 assertions |
| `--socket-output` | queued DATA and small positive-size file output | 9 cases / 78 assertions |
| `--blocking` | blocking Socket I/O, Rust-owned IOVector | 7 cases / 54 assertions |

Fixtures use private socketpairs and temporary files, consume every byte and
keep peers live until normal cleanup. They do not probe errors, interruption,
zero-length I/O, lifetime or SSL. The 9/78 socket-output inventory passes on
macOS and Linux for both engines; the owned-storage profiles pass on macOS.

## Capability modifiers

Modifiers add identity checks, not assertions. The owned-storage modifiers
compare the selected engine (Rust storage) with
the Native control.

| Modifier | Profile | Checks |
| --- | --- | --- |
| `--require-owned-defer` | `--builtins` | defer import, field types and push/step/clear origins |
| `--require-output-workflow` | `--socket-output` | exact workflow source and caller-overlay hashes, selected workflow symbol origin |
| `--require-typed-buffer` | any | typed Buffer request/SDK/source; Buffer execution only in `--socket-output` |
| `--require-batch` | any | refuses older production sources before compiling |

The owned-IOV profile checks that the IOVector constructor/destructor and the
Rust-owned length symbol resolve to the selected engine (Native must not import
it). Its kernel `writev` case uses raw input descriptors, so it does not exercise
a kernel call over owned spans. The owned-defer profile checks the public defer
pointer and embedded destroy-queue types and the push/step/clear origins; it
does not run the destroy callback or Reactor destruction.
