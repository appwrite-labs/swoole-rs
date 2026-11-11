# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `6f80f8e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,419 | 15,210 | 0.9480× | 0.94× | ✅ |
| echo-1k | 17,549 | 18,162 | 0.9663× | 0.97× | ✅ |
| json | 22,496 | 23,170 | 0.9709× | 0.97× | ✅ |
| service-health | 16,990 | 17,782 | 0.9555× | 0.95× | ✅ |
| websocket | 6,863 | 8,191 | 0.8379× | 0.84× | ✅ |
| timer | 202,391 | 313,560 | 0.6455× | 0.65× | ✅ |
| app | 126,337 | 135,860 | 0.9299× | 0.94× | ✅ |
| http2 | 20,255 | 20,463 | 0.9898× | 0.99× | ✅ |
| table | 196,141 | 201,874 | 0.9716× | 0.98× | ✅ |
| channel | 139,687 | 154,664 | 0.9032× | 0.91× | ✅ |
| task | 32,260 | 33,265 | 0.9698× | 0.97× | ✅ |
| mysql | 49,913 | 51,061 | 0.9775× | 0.98× | ✅ |
| http-client | 34,044 | 35,829 | 0.9502× | 0.96× | ✅ |
| client | 18,829 | 19,766 | 0.9526× | 0.95× | ✅ |
| socket | 20,230 | 21,070 | 0.9601× | 0.97× | ✅ |
| gzip | 196,406 | 197,079 | 0.9966× | 1.0× | ✅ |
| websocket-deflate | 181,633 | 181,572 | 1.0003× | 1.0× | ❌ |
| tcp-length | 5,373 | 4,762 | 1.1283× | 1.13× | ➖ |
| tcp-eof | 5,929 | 10,228 | 0.5797× | 0.58× | ✅ |
| udp | 8,019 | 7,986 | 1.0041× | 1.0× | ❌ |
| connect | 18,803 | 18,707 | 1.0052× | 0.99× | ❌ |
| https | 109,551 | 108,810 | 1.0068× | 1.01× | ➖ |
| wss | 104,993 | 104,802 | 1.0018× | 1.0× | ❌ |
| atomic | 50,206 | 51,325 | 0.9782× | 0.98× | ✅ |
| process-mode | 16,694 | 16,959 | 0.9844× | 0.98× | ✅ |
| http2-concurrent | 378,909 | 409,069 | 0.9263× | 0.92× | ✅ |
| websocket-64k | 156,307 | 159,700 | 0.9788× | 0.98× | ✅ |
| websocket-fragmented | 13,172 | 16,339 | 0.8062× | 0.79× | ✅ |
| upload | 675,471 | 1,322,380 | 0.5108× | 0.51× | ✅ |
| static | 34,641 | 41,016 | 0.8446× | 0.85× | ✅ |
| pgsql | 23,679 | 24,594 | 0.9628× | 0.97× | ✅ |
| sqlite | 23,998 | 25,217 | 0.9517× | 0.96× | ✅ |
| curl | 85,970 | 90,381 | 0.9512× | 0.96× | ✅ |
| dns | 231,015 | 233,095 | 0.9911× | 1.0× | ✅ |

Geometric mean 0.9174×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
