# Worker lifecycle consumers

`worker_retirement.py --core <core_tests.py build root> --output <report.json>`
compiles `worker_retirement.cc` against the actual Native and hybrid Server SDKs
and static libraries, in C++14 and C++17. It checks direct PID writes, first-match
lookup, native session bitfields (including the final slot and reserved bits),
and worker/shared concurrency reset over the real Server allocation. The
unchanged original C++ and PHPT inventories remain separate proofs.
