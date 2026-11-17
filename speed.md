# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `932415e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,794 | 15,214 | 0.9067× | 0.91× | ✅ |
| echo-1k | 16,811 | 18,165 | 0.9254× | 0.93× | ✅ |
| json | 21,957 | 23,161 | 0.9480× | 0.95× | ✅ |
| service-health | 16,366 | 17,777 | 0.9207× | 0.92× | ✅ |
| websocket | 6,554 | 8,188 | 0.8005× | 0.8× | ✅ |
| timer | 205,904 | 313,592 | 0.6566× | 0.65× | ✅ |
| app | 126,895 | 135,848 | 0.9341× | 0.93× | ✅ |
| http2 | 19,147 | 20,468 | 0.9355× | 0.92× | ✅ |
| table | 194,822 | 201,865 | 0.9651× | 0.97× | ✅ |
| channel | 137,575 | 155,170 | 0.8866× | 0.88× | ✅ |
| task | 32,496 | 33,261 | 0.9770× | 0.96× | ✅ |
| mysql | 49,983 | 51,032 | 0.9795× | 0.97× | ✅ |
| http-client | 33,718 | 35,804 | 0.9417× | 0.93× | ✅ |
| gzip | 194,466 | 197,083 | 0.9867× | 0.99× | ✅ |
| websocket-deflate | 178,999 | 181,572 | 0.9858× | 0.99× | ✅ |
| tcp-length | 4,586 | 4,762 | 0.9630× | 0.96× | ✅ |
| tcp-eof | 5,166 | 10,231 | 0.5049× | 0.5× | ✅ |
| udp | 7,487 | 7,987 | 0.9375× | 0.93× | ✅ |
| connect | 17,505 | 18,703 | 0.9359× | 0.93× | ✅ |
| https | 105,959 | 108,813 | 0.9738× | 0.98× | ✅ |
| wss | 100,988 | 104,797 | 0.9637× | 0.97× | ✅ |
| atomic | 49,865 | 51,316 | 0.9717× | 0.97× | ✅ |
| process-mode | 15,047 | 16,957 | 0.8874× | 0.88× | ✅ |
| http2-concurrent | 366,035 | 409,073 | 0.8948× | 0.88× | ✅ |
| websocket-64k | 89,809 | 159,694 | 0.5624× | 0.57× | ✅ |
| websocket-fragmented | 11,625 | 16,327 | 0.7120× | 0.72× | ✅ |
| upload | 415,387 | 1,322,389 | 0.3141× | 0.32× | ✅ |
| static | 26,690 | 41,080 | 0.6497× | 0.65× | ✅ |
| client | 18,613 | 19,756 | 0.9421× | 0.94× | ✅ |
| socket | 19,754 | 21,072 | 0.9375× | 0.92× | ✅ |
| pgsql | 22,822 | 24,594 | 0.9279× | 0.93× | ✅ |
| sqlite | 23,460 | 25,236 | 0.9296× | 0.93× | ✅ |
| curl | 86,419 | 90,388 | 0.9561× | 0.94× | ✅ |
| dns | 230,099 | 232,552 | 0.9895× | 0.99× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
