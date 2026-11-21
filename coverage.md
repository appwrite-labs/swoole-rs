# Coverage of crates/

Lines of `crates/` the Cargo swoole.so ran under the PHPT corpus, the corpus apps and swoole/library's suite, measured on `70426a5` (`tools/coverage.py`, nightly `.github/workflows/coverage.yml`). A process that exits by a signal or `_exit` records nothing.

| crate | lines | covered | entry points not reached |
| --- | ---: | ---: | ---: |
| phpthread | 1,647 | 0.0% | 52 |
| base64 | 84 | 35.7% | 0 |
| odbc | 1,034 | 39.7% | 0 |
| pgsql | 2,156 | 42.8% | 9 |
| crc32 | 41 | 46.3% | 0 |
| error-ffi | 114 | 54.4% | 6 |
| phplocks | 441 | 58.7% | 17 |
| error | 120 | 62.5% | 0 |
| phpredis | 195 | 66.7% | 0 |
| ports | 1,528 | 67.0% | 0 |
| protocols | 399 | 67.4% | 0 |
| phpevents | 737 | 67.7% | 0 |
| sqlite | 379 | 69.7% | 0 |
| curl | 2,445 | 72.0% | 6 |
| phpruntime | 2,592 | 72.1% | 1 |
| phpprocess | 2,011 | 72.9% | 2 |
| server | 10,830 | 73.8% | 0 |
| phpcoroutines | 2,847 | 74.4% | 2 |
| clients | 2,821 | 74.5% | 0 |
| corebase | 21,325 | 75.2% | 3 |
| http | 4,208 | 75.5% | 0 |
| zend | 3,669 | 76.6% | 5 |
| phpswoole | 1,614 | 79.1% | 0 |
| hooks | 4,414 | 79.3% | 1 |
| phpserver | 4,596 | 82.3% | 1 |
| phpclients | 7,074 | 82.4% | 0 |
| bus | 567 | 84.5% | 0 |
| phpcollections | 557 | 85.3% | 0 |
| phptimer | 165 | 85.5% | 0 |
| websocket | 403 | 85.6% | 0 |
| phphttp2 | 1,139 | 86.0% | 0 |
| inflate | 96 | 86.5% | 0 |
| phphttp | 5,823 | 87.3% | 0 |
| framing | 349 | 88.5% | 0 |
| brotli | 75 | 89.3% | 0 |
| httpbindings | 239 | 90.4% | 0 |
| http2 | 763 | 91.2% | 0 |
| deflate | 57 | 91.2% | 0 |
| ssh2 | 85 | 92.9% | 0 |
| zlib | 95 | 93.7% | 0 |
| oci | 137 | 94.2% | 0 |
| firebird | 50 | 96.0% | 0 |
| locale | 116 | 99.1% | 0 |
| curl-sys | 10 | 100.0% | 0 |
| ext | 3 | 100.0% | 0 |
| ffi | 20 | 100.0% | 0 |
| phpstubs | 12 | 100.0% | 0 |
| **crates/** | 90,082 | 74.5% | 105 |

# Entry points no run reached

`zif_`/`zim_` handlers and `swr_v1_` exports whose counter stayed zero, per crate: candidates for targeted differential tests ([#2249](https://github.com/appwrite-labs/swoole-rs/issues/2249)).

## corebase

- `swr_v1_core_format_result`
- `swr_v1_coroutine_scheduler_get`
- `swr_v1_logger_put`

## curl

- `swr_v1_curl_get_handle`
- `swr_v1_curl_handle_leave_multi`
- `swr_v1_curl_multi_free`
- `zif_swoole_native_curl_escape`
- `zif_swoole_native_curl_unescape`
- `zif_swoole_native_curl_upkeep`

## error-ffi

- `swr_v1_abi_version`
- `swr_v1_error_buffer`
- `swr_v1_error_message`
- `swr_v1_error_resolve`
- `swr_v1_error_strerror`
- `swr_v1_feature_bits`

## hooks

- `swr_v1_hook_co_socket_uring`

## pgsql

- `zim_PDO_PGSql_Ext_pgsqlCopyFromArray`
- `zim_PDO_PGSql_Ext_pgsqlCopyFromFile`
- `zim_PDO_PGSql_Ext_pgsqlCopyToArray`
- `zim_PDO_PGSql_Ext_pgsqlCopyToFile`
- `zim_PDO_PGSql_Ext_pgsqlGetNotify`
- `zim_PDO_PGSql_Ext_pgsqlGetPid`
- `zim_PDO_PGSql_Ext_pgsqlLOBCreate`
- `zim_PDO_PGSql_Ext_pgsqlLOBOpen`
- `zim_PDO_PGSql_Ext_pgsqlLOBUnlink`

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

## zend

- `swr_v1_curl_check_handle`
- `swr_v1_phphelpers_class_alias`
- `swr_v1_phpruntime_plain_fdopen_mode`
- `swr_v1_phpruntime_plain_metadata`
- `swr_v1_phpruntime_plain_rename`
