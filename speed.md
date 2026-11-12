# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c27c130` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,900 | 15,210 | 0.9139× | 0.92× | ✅ |
| echo-1k | 17,022 | 18,162 | 0.9373× | 0.94× | ✅ |
| json | 21,984 | 23,158 | 0.9493× | 0.96× | ✅ |
| service-health | 16,488 | 17,777 | 0.9275× | 0.93× | ✅ |
| websocket | 6,719 | 8,188 | 0.8206× | 0.82× | ✅ |
| timer | 202,192 | 313,585 | 0.6448× | 0.65× | ✅ |
| app | 125,965 | 135,933 | 0.9267× | 0.93× | ✅ |
| http2 | 19,242 | 20,464 | 0.9403× | 0.97× | ✅ |
| table | 195,696 | 201,869 | 0.9694× | 0.97× | ✅ |
| channel | 138,843 | 154,654 | 0.8978× | 0.9× | ✅ |
| task | 31,928 | 33,267 | 0.9597× | 0.96× | ✅ |
| mysql | 49,472 | 51,061 | 0.9689× | 0.97× | ✅ |
| http-client | 33,564 | 35,805 | 0.9374× | 0.94× | ✅ |
| client | 18,439 | 19,757 | 0.9333× | 0.94× | ✅ |
| socket | 19,719 | 21,067 | 0.9360× | 0.92× | ✅ |
| gzip | 195,981 | 197,083 | 0.9944× | 1.0× | ✅ |
| websocket-deflate | 181,474 | 181,568 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,023 | 4,765 | 1.0540× | 1.05× | ❌ |
| tcp-eof | 5,565 | 10,231 | 0.5439× | 0.54× | ✅ |
| udp | 8,055 | 7,986 | 1.0086× | 1.0× | ❌ |
| connect | 18,246 | 18,707 | 0.9754× | 0.98× | ✅ |
| https | 108,754 | 108,813 | 0.9995× | 1.01× | ✅ |
| wss | 104,591 | 104,800 | 0.9980× | 1.0× | ✅ |
| atomic | 49,647 | 51,316 | 0.9675× | 0.97× | ✅ |
| process-mode | 16,395 | 16,959 | 0.9667× | 0.97× | ✅ |
| http2-concurrent | 357,869 | 409,573 | 0.8738× | 0.91× | ✅ |
| websocket-64k | 156,167 | 159,697 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,079 | 16,327 | 0.8011× | 0.79× | ✅ |
| upload | 544,634 | 1,322,382 | 0.4119× | 0.42× | ✅ |
| static | 34,434 | 41,045 | 0.8389× | 0.84× | ✅ |
| pgsql | 23,189 | 24,595 | 0.9429× | 0.95× | ✅ |
| sqlite | 23,469 | 25,232 | 0.9301× | 0.94× | ✅ |
| curl | 85,299 | 90,400 | 0.9436× | 0.95× | ✅ |
| dns | 231,220 | 233,084 | 0.9920× | 1.0× | ✅ |

Geometric mean 0.8954×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
