# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e82e786` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,924 | 15,214 | 0.9152× | 0.92× | ✅ |
| echo-1k | 17,072 | 18,162 | 0.9400× | 0.94× | ✅ |
| json | 22,025 | 23,158 | 0.9511× | 0.96× | ✅ |
| service-health | 16,516 | 17,777 | 0.9291× | 0.93× | ✅ |
| websocket | 6,719 | 8,188 | 0.8206× | 0.82× | ✅ |
| timer | 202,539 | 313,599 | 0.6459× | 0.65× | ✅ |
| app | 125,976 | 135,773 | 0.9278× | 0.93× | ✅ |
| http2 | 19,818 | 20,464 | 0.9684× | 0.97× | ✅ |
| table | 195,729 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,873 | 154,655 | 0.8980× | 0.9× | ✅ |
| task | 31,967 | 33,265 | 0.9610× | 0.96× | ✅ |
| mysql | 49,543 | 51,055 | 0.9704× | 0.97× | ✅ |
| http-client | 33,619 | 35,806 | 0.9389× | 0.94× | ✅ |
| client | 18,473 | 19,757 | 0.9350× | 0.94× | ✅ |
| socket | 19,752 | 21,067 | 0.9376× | 0.92× | ✅ |
| gzip | 196,003 | 197,083 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,477 | 181,572 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,023 | 4,762 | 1.0547× | 1.05× | ❌ |
| tcp-eof | 5,565 | 10,231 | 0.5439× | 0.54× | ✅ |
| udp | 8,037 | 7,986 | 1.0064× | 1.0× | ❌ |
| connect | 18,251 | 18,707 | 0.9757× | 0.98× | ✅ |
| https | 108,832 | 108,810 | 1.0002× | 1.01× | ➖ |
| wss | 104,665 | 104,797 | 0.9987× | 1.0× | ✅ |
| atomic | 49,680 | 51,316 | 0.9681× | 0.97× | ✅ |
| process-mode | 16,426 | 16,959 | 0.9685× | 0.97× | ✅ |
| http2-concurrent | 372,419 | 408,881 | 0.9108× | 0.91× | ✅ |
| websocket-64k | 156,162 | 159,699 | 0.9778× | 0.98× | ✅ |
| websocket-fragmented | 13,079 | 16,328 | 0.8011× | 0.79× | ✅ |
| upload | 544,887 | 1,322,381 | 0.4121× | 0.42× | ✅ |
| static | 34,448 | 41,045 | 0.8393× | 0.84× | ✅ |
| pgsql | 23,213 | 24,594 | 0.9438× | 0.95× | ✅ |
| sqlite | 23,493 | 25,232 | 0.9311× | 0.94× | ✅ |
| curl | 85,324 | 90,382 | 0.9440× | 0.95× | ✅ |
| dns | 230,725 | 232,514 | 0.9923× | 1.0× | ✅ |

Geometric mean 0.8980×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
