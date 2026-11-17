# Coverage of crates/

Lines of `crates/` the Cargo swoole.so ran under the PHPT corpus, the corpus apps and swoole/library's suite, measured on `58ccfc7` (`tools/coverage.py`, nightly `.github/workflows/coverage.yml`). A process that exits by a signal or `_exit` records nothing.

| crate | lines | covered | entry points not reached |
| --- | ---: | ---: | ---: |
| phpthread | 1,646 | 0.0% | 52 |
| base64 | 84 | 35.7% | 0 |
| utilities | 724 | 37.0% | 1 |
| error-ffi | 69 | 39.1% | 6 |
| crc32 | 40 | 47.5% | 0 |
| phplocks | 441 | 59.2% | 17 |
| error | 120 | 62.5% | 0 |
| odbc | 185 | 64.9% | 0 |
| protocols | 399 | 67.4% | 0 |
| phpredis | 200 | 67.5% | 0 |
| phpevents | 729 | 67.8% | 0 |
| corebase | 6,161 | 71.5% | 3 |
| server | 10,409 | 72.4% | 0 |
| ports | 1,989 | 72.6% | 0 |
| phpprocess | 2,012 | 73.2% | 2 |
| clients | 2,450 | 73.2% | 0 |
| tls | 2,843 | 73.3% | 0 |
| phpcoroutines | 2,849 | 74.8% | 2 |
| http | 3,708 | 74.9% | 0 |
| reactor | 3,633 | 76.0% | 0 |
| process | 4,100 | 76.2% | 0 |
| zend | 3,290 | 76.9% | 0 |
| phpruntime | 1,892 | 77.8% | 1 |
| memory | 1,563 | 78.5% | 0 |
| phpswoole | 1,629 | 79.4% | 0 |
| brotli | 73 | 79.5% | 0 |
| hooks | 5,041 | 80.0% | 1 |
| timer | 1,082 | 80.2% | 0 |
| phpserver | 4,589 | 81.3% | 1 |
| http-ffi | 459 | 81.7% | 0 |
| phpclients | 6,567 | 81.8% | 0 |
| bus | 544 | 84.2% | 0 |
| phptimer | 160 | 84.4% | 0 |
| pgsql | 318 | 84.9% | 0 |
| sqlite | 80 | 85.0% | 0 |
| phpcollections | 561 | 85.2% | 0 |
| ffi | 68 | 85.3% | 1 |
| websocket | 402 | 85.6% | 0 |
| inflate | 96 | 86.5% | 0 |
| phphttp | 5,085 | 87.0% | 0 |
| phphttp2 | 2,439 | 87.0% | 0 |
| framing | 349 | 88.5% | 0 |
| httpbindings | 224 | 90.2% | 0 |
| zlib | 99 | 90.9% | 0 |
| http2 | 720 | 91.0% | 0 |
| deflate | 58 | 91.4% | 0 |
| curl | 680 | 92.6% | 1 |
| ssh2 | 85 | 92.9% | 0 |
| firebird | 30 | 93.3% | 0 |
| oci | 125 | 94.4% | 0 |
| locale | 114 | 99.1% | 0 |
| curl-sys | 10 | 100.0% | 0 |
| ext | 3 | 100.0% | 0 |
| **crates/** | 83,226 | 75.6% | 88 |

# Entry points no run reached

`zif_`/`zim_` handlers and `swr_v1_` exports whose counter stayed zero, per crate: candidates for targeted differential tests ([#2249](https://github.com/appwrite-labs/swoole-rs/issues/2249)).

## corebase

- `swr_v1_core_format_result`
- `swr_v1_locks_atomic32`
- `swr_v1_locks_atomic64`

## curl

- `swr_v1_curl_multi_free`

## error-ffi

- `swr_v1_abi_version`
- `swr_v1_error_buffer`
- `swr_v1_error_message`
- `swr_v1_error_resolve`
- `swr_v1_error_strerror`
- `swr_v1_feature_bits`

## ffi

- `swr_v1_ffi_exit`

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

## utilities

- `swr_v1_logger_put`
