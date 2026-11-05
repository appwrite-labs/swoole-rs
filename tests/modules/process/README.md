# Worker lifecycle consumers

`worker_retirement.py --core <core_tests.py build root> --output <report.json>`
compiles `worker_retirement.cc` against the actual Native and hybrid Server SDKs
and core libraries, in C++14 and C++17. It checks direct PID writes, first-match
lookup, native session bitfields (including the final slot and reserved bits),
and worker/shared concurrency reset over the real Server allocation. The
unchanged original C++ and PHPT inventories remain separate proofs.

The Server startup draft uses the existing typed Server Host rather than a
continuation interpreter. Mode retains its complete native enum word in Rust's
configuration; an unconfigured external Host (mode zero) retains the legacy
mode-fact callback. Native constructors publish their actual enum value, including
the operationally invalid zero value. The native enum getter and word-width
assertion remain. Start claim/timestamp/running state are canonical Rust fields;
callbacks, daemon/fork, resource creation, diagnostics and PID I/O stay native.
The M08 startup override follows worker publication; graduated-only startup stays
upstream. Pure result/order checks do not qualify the complete PHP C++ failure
entry chain; production activation is gated on #1230/#1145.
