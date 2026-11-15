# Coverage of crates/

Lines of `crates/` the Cargo swoole.so ran under the PHPT corpus, the corpus apps and swoole/library's suite, measured on `ed4e8f4` (`tools/coverage.py`, nightly `.github/workflows/coverage.yml`). A process that exits by a signal or `_exit` records nothing.

| crate | lines | covered | entry points not reached |
| --- | ---: | ---: | ---: |
| phpthread | 1,675 | 0.0% | 52 |
| websocket-ffi | 147 | 16.3% | 0 |
| utilities | 949 | 29.5% | 17 |
| base64 | 99 | 33.3% | 0 |
| error-ffi | 69 | 40.6% | 6 |
| crc32 | 42 | 45.2% | 0 |
| protocols-ffi | 233 | 51.1% | 0 |
| phplocks | 447 | 60.9% | 17 |
| buffer | 751 | 64.8% | 0 |
| corebase | 2,447 | 66.7% | 10 |
| odbc | 178 | 66.9% | 0 |
| phpevents | 729 | 67.2% | 0 |
| protocols | 409 | 67.5% | 0 |
| phpredis | 202 | 67.8% | 0 |
| error | 122 | 71.3% | 0 |
| locks | 410 | 72.0% | 4 |
| http-ffi | 562 | 72.1% | 0 |
| ossupport | 742 | 72.4% | 0 |
| coroutine | 1,146 | 72.4% | 0 |
| server | 10,164 | 73.1% | 2 |
| phpcoroutines | 2,859 | 74.9% | 2 |
| tls | 2,712 | 74.9% | 2 |
| clients | 2,175 | 74.9% | 1 |
| http | 3,839 | 75.0% | 0 |
| phpprocess | 2,020 | 75.4% | 2 |
| phpruntime | 1,995 | 78.1% | 1 |
| reactor | 3,653 | 78.6% | 5 |
| process | 4,056 | 79.5% | 1 |
| timer | 1,061 | 79.5% | 1 |
| ports | 1,776 | 79.8% | 0 |
| memory | 1,629 | 80.2% | 0 |
| firebird | 51 | 80.4% | 0 |
| phpswoole | 1,675 | 80.5% | 0 |
| zend | 3,147 | 80.7% | 0 |
| pgsql | 368 | 81.2% | 0 |
| hooks | 4,816 | 82.0% | 1 |
| ssh2 | 139 | 82.0% | 0 |
| phpclients | 6,593 | 82.5% | 0 |
| phptimer | 171 | 83.0% | 0 |
| phpserver | 4,577 | 83.4% | 1 |
| brotli | 64 | 84.4% | 0 |
| sqlite | 80 | 85.0% | 0 |
| phpcollections | 574 | 86.2% | 0 |
| bus | 500 | 87.6% | 0 |
| phphttp2 | 2,362 | 88.0% | 0 |
| inflate | 98 | 88.8% | 0 |
| phphttp | 5,122 | 88.9% | 0 |
| zlib | 102 | 89.2% | 0 |
| httpbindings | 250 | 90.0% | 0 |
| framing | 375 | 90.4% | 0 |
| ffi | 63 | 90.5% | 1 |
| http2 | 715 | 91.3% | 0 |
| deflate | 61 | 91.8% | 0 |
| curl | 616 | 92.4% | 0 |
| websocket | 311 | 94.9% | 0 |
| oci | 126 | 95.2% | 0 |
| curl-sys | 10 | 100.0% | 0 |
| ext | 3 | 100.0% | 0 |
| locale | 61 | 100.0% | 0 |
| **crates/** | 82,328 | 76.4% | 126 |

# Entry points no run reached

`zif_`/`zim_` handlers and `swr_v1_` exports whose counter stayed zero, per crate: candidates for targeted differential tests ([#2249](https://github.com/appwrite-labs/swoole-rs/issues/2249)).

## clients

- `swr_v1_clients_new`

## corebase

- `swr_v1_core_format_result`
- `swr_v1_core_last_error`
- `swr_v1_core_proxy_free`
- `swr_v1_core_proxy_new`
- `swr_v1_core_signal_alarm`
- `swr_v1_core_string_clone`
- `swr_v1_core_string_free`
- `swr_v1_core_string_set`
- `swr_v1_core_string_view`
- `swr_v1_core_trace_flags`

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

## locks

- `swr_v1_locks_atomic32`
- `swr_v1_locks_atomic64`
- `swr_v1_locks_wait`
- `swr_v1_locks_wakeup`

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

## process

- `swr_v1_worker_globals`

## reactor

- `swr_v1_reactor_new`
- `swr_v1_reactor_registry_count`
- `swr_v1_reactor_registry_get`
- `swr_v1_reactor_set_end`
- `swr_v1_socket_default_timeouts`

## server

- `swr_v1_server_connection`
- `swr_v1_server_session`

## timer

- `swr_v1_timer_add_plain`

## tls

- `swr_v1_tls_owner_free`
- `swr_v1_tls_owner_new`

## utilities

- `swr_v1_logger_close`
- `swr_v1_logger_file`
- `swr_v1_logger_free`
- `swr_v1_logger_is_opened`
- `swr_v1_logger_level`
- `swr_v1_logger_new`
- `swr_v1_logger_open`
- `swr_v1_logger_put`
- `swr_v1_logger_reopen`
- `swr_v1_logger_rotation_date`
- `swr_v1_logger_set`
- `swr_v1_logger_set_date_format`
- `swr_v1_logger_set_level`
- `swr_v1_logger_set_stream`
- `swr_v1_utils_pretty_range`
- `swr_v1_utils_wait`
- `swr_v1_utils_wakeup`
