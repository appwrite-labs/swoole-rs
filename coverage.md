# Coverage of crates/

Lines of `crates/` the Cargo swoole.so ran under the PHPT corpus, the corpus apps and swoole/library's suite, measured on `4f22d1e` (`tools/coverage.py`, nightly `.github/workflows/coverage.yml`). A process that exits by a signal or `_exit` records nothing.

| crate | lines | covered | entry points not reached |
| --- | ---: | ---: | ---: |
| phpthread | 1,647 | 0.0% | 52 |
| base64 | 84 | 35.7% | 0 |
| crc32 | 40 | 47.5% | 0 |
| error-ffi | 115 | 55.7% | 6 |
| phplocks | 438 | 58.7% | 17 |
| error | 120 | 62.5% | 0 |
| odbc | 185 | 64.9% | 0 |
| phpredis | 195 | 66.7% | 0 |
| protocols | 399 | 67.4% | 0 |
| phpevents | 729 | 67.5% | 0 |
| ports | 1,844 | 71.8% | 0 |
| phpprocess | 2,014 | 72.9% | 2 |
| server | 10,872 | 73.3% | 0 |
| clients | 2,518 | 73.3% | 0 |
| phpcoroutines | 2,866 | 74.2% | 2 |
| corebase | 20,988 | 75.1% | 4 |
| http | 4,154 | 75.9% | 0 |
| zend | 3,474 | 76.9% | 0 |
| phpruntime | 1,869 | 77.5% | 1 |
| phpswoole | 1,614 | 79.1% | 0 |
| hooks | 4,314 | 79.3% | 1 |
| phpclients | 6,545 | 81.7% | 0 |
| phpserver | 4,607 | 82.5% | 1 |
| bus | 553 | 84.1% | 0 |
| pgsql | 312 | 84.6% | 0 |
| sqlite | 80 | 85.0% | 0 |
| phpcollections | 557 | 85.1% | 0 |
| phptimer | 165 | 85.5% | 0 |
| websocket | 402 | 85.6% | 0 |
| inflate | 96 | 86.5% | 0 |
| phphttp2 | 2,416 | 86.5% | 0 |
| phphttp | 5,151 | 87.0% | 0 |
| framing | 349 | 88.8% | 0 |
| brotli | 75 | 89.3% | 0 |
| httpbindings | 224 | 90.2% | 0 |
| http2 | 740 | 91.2% | 0 |
| deflate | 58 | 91.4% | 0 |
| curl | 698 | 92.6% | 1 |
| ssh2 | 85 | 92.9% | 0 |
| zlib | 95 | 93.7% | 0 |
| oci | 125 | 94.4% | 0 |
| firebird | 50 | 96.0% | 0 |
| locale | 116 | 99.1% | 0 |
| curl-sys | 10 | 100.0% | 0 |
| ext | 3 | 100.0% | 0 |
| ffi | 15 | 100.0% | 0 |
| phpstubs | 12 | 100.0% | 0 |
| **crates/** | 84,018 | 76.1% | 87 |

# Entry points no run reached

`zif_`/`zim_` handlers and `swr_v1_` exports whose counter stayed zero, per crate: candidates for targeted differential tests ([#2249](https://github.com/appwrite-labs/swoole-rs/issues/2249)).

## corebase

- `swr_v1_core_format_result`
- `swr_v1_locks_atomic32`
- `swr_v1_locks_atomic64`
- `swr_v1_logger_put`

## curl

- `swr_v1_curl_multi_free`

## error-ffi

- `swr_v1_abi_version`
- `swr_v1_error_buffer`
- `swr_v1_error_message`
- `swr_v1_error_resolve`
- `swr_v1_error_strerror`
- `swr_v1_feature_bits`

## hooks

- `swr_v1_hook_co_socket_uring`

## phpcoroutines

- `zif_swoole_exit`
- `zim_swoole_coroutine_getExecuteTime`

## phplocks

- `zim_swoole_thread_atomic___construct`
- `zim_swoole_thread_atomic_add`
- `zim_swoole_thread_atomic_cmpset`
- `zim_swoole_thread_atomic_get`
- `zim_swoole_thread_atomic_long___construct`
- `zim_swoole_thread_atomic_long_add`
- `zim_swoole_thread_atomic_long_cmpset`
- `zim_swoole_thread_atomic_long_get`
- `zim_swoole_thread_atomic_long_set`
- `zim_swoole_thread_atomic_long_sub`
- `zim_swoole_thread_atomic_set`
- `zim_swoole_thread_atomic_sub`
- `zim_swoole_thread_atomic_wait`
- `zim_swoole_thread_atomic_wakeup`
- `zim_swoole_thread_lock___construct`
- `zim_swoole_thread_lock_lock`
- `zim_swoole_thread_lock_unlock`

## phpprocess

- `zim_swoole_process_exec`
- `zim_swoole_process_pool_stop`

## phpruntime

- `zif_swoole_putenv`

## phpserver

- `zim_swoole_server_port___construct`

## phpthread

- `zim_swoole_thread___construct`
- `zim_swoole_thread_activeCount`
- `zim_swoole_thread_arraylist___construct`
- `zim_swoole_thread_arraylist_clean`
- `zim_swoole_thread_arraylist_count`
- `zim_swoole_thread_arraylist_decr`
- `zim_swoole_thread_arraylist_find`
- `zim_swoole_thread_arraylist_incr`
- `zim_swoole_thread_arraylist_offsetExists`
- `zim_swoole_thread_arraylist_offsetGet`
- `zim_swoole_thread_arraylist_offsetSet`
- `zim_swoole_thread_arraylist_offsetUnset`
- `zim_swoole_thread_arraylist_sort`
- `zim_swoole_thread_arraylist_toArray`
- `zim_swoole_thread_barrier___construct`
- `zim_swoole_thread_barrier_wait`
- `zim_swoole_thread_detach`
- `zim_swoole_thread_getAffinity`
- `zim_swoole_thread_getArguments`
- `zim_swoole_thread_getExitStatus`
- `zim_swoole_thread_getId`
- `zim_swoole_thread_getInfo`
- `zim_swoole_thread_getNativeId`
- `zim_swoole_thread_getPriority`
- `zim_swoole_thread_isAlive`
- `zim_swoole_thread_join`
- `zim_swoole_thread_joinable`
- `zim_swoole_thread_map___construct`
- `zim_swoole_thread_map_add`
- `zim_swoole_thread_map_clean`
- `zim_swoole_thread_map_count`
- `zim_swoole_thread_map_decr`
- `zim_swoole_thread_map_find`
- `zim_swoole_thread_map_incr`
- `zim_swoole_thread_map_keys`
- `zim_swoole_thread_map_offsetExists`
- `zim_swoole_thread_map_offsetGet`
- `zim_swoole_thread_map_offsetSet`
- `zim_swoole_thread_map_offsetUnset`
- `zim_swoole_thread_map_sort`
- `zim_swoole_thread_map_toArray`
- `zim_swoole_thread_map_update`
- `zim_swoole_thread_map_values`
- `zim_swoole_thread_queue___construct`
- `zim_swoole_thread_queue_clean`
- `zim_swoole_thread_queue_count`
- `zim_swoole_thread_queue_pop`
- `zim_swoole_thread_queue_push`
- `zim_swoole_thread_setAffinity`
- `zim_swoole_thread_setName`
- `zim_swoole_thread_setPriority`
- `zim_swoole_thread_yield`
