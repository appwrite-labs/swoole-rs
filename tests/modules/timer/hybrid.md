# Selected native timer adapter gate

`test_adapter.py` compiles the shared `probe.cc` against the selected build's
headers into a native probe loaded through PHP FFI. The probe takes the Timer,
the `swoole_timer_*` wrappers and the Rust queue from the loaded extension, so it
tests the shipped timer, whichever source defines it. macOS or Linux:

```sh
python3 -m pytest tests/modules/timer/test_adapter.py --sanitize \
  --manifest target/hybrid/rust/build-manifest.json
```

Before running, it rejects a mutable build, wrong upstream pin, missing timer
module, and any mismatch between the manifest and the extension, Rust archive,
source diff or ABI headers. Module defines come from every module in the
manifest, so timer-only and combined builds both work. Linux loads shared PHP FFI
if it is not built in; `--php-extension` adds other modules.

There are 14 scenarios, each in an isolated process group with a five-second
limit. Every process must exit 0 and produce exactly the expected final
observation:

- equal-deadline A/C/B, self-remove, remove-other, destructor-remove-other,
  active repeating update, destructor-add, fork teardown;
- active one-shot delay (`AaBb`) and recursive selection (`Aa`) without
  reentering the active timer;
- `fork-active-teardown`: the child deletes the copied owner while A is active
  and exits without resuming the inherited selection; the parent then drains
  both nodes (`AaBb`, two destructors);
- callback throws, destructor throws, both: the primary exception survives, the
  fired token is destroyed once, and the other timer still dispatches;
- `wrapper-clear`: real `swoole_timer_clear(id)` for inactive, self, other,
  repeated and missing IDs, with exactly three destructions.

`--sanitize` instruments the probe with ASan/UBSan (leak
checking off); the extension, Rust and PHP are not instrumented. Any sanitizer log
fails the gate.

## Known failures

Linux `--sanitize` is unavailable: ASan rejects Debian PHP's `RTLD_DEEPBIND`
loading of the shared FFI module, so every PHP process exits 86 before the probe
runs. Run Linux unsanitized; macOS passes 14/14 sanitized.
