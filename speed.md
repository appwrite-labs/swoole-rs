# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `1a0f899` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,901 | 15,214 | 0.9137× | 0.92× | ✅ |
| echo-1k | 17,000 | 18,162 | 0.9360× | 0.94× | ✅ |
| json | 22,056 | 23,161 | 0.9523× | 0.95× | ✅ |
| service-health | 16,493 | 17,777 | 0.9278× | 0.93× | ✅ |
| websocket | 6,561 | 8,188 | 0.8014× | 0.8× | ✅ |
| timer | 206,144 | 313,531 | 0.6575× | 0.65× | ✅ |
| app | 125,392 | 135,878 | 0.9228× | 0.93× | ✅ |
| http2 | 18,894 | 20,468 | 0.9231× | 0.91× | ✅ |
| table | 195,056 | 201,865 | 0.9663× | 0.97× | ✅ |
| channel | 136,684 | 155,170 | 0.8809× | 0.88× | ✅ |
| task | 32,483 | 33,268 | 0.9764× | 0.96× | ✅ |
| mysql | 49,541 | 51,058 | 0.9703× | 0.97× | ✅ |
| http-client | 33,076 | 35,803 | 0.9238× | 0.94× | ✅ |
| gzip | 194,123 | 197,083 | 0.9850× | 0.99× | ✅ |
| websocket-deflate | 178,895 | 181,572 | 0.9853× | 0.99× | ✅ |
| tcp-length | 4,650 | 4,765 | 0.9758× | 0.99× | ✅ |
| tcp-eof | 5,145 | 10,228 | 0.5030× | 0.51× | ✅ |
| udp | 7,486 | 7,987 | 0.9373× | 0.93× | ✅ |
| connect | 17,617 | 18,707 | 0.9417× | 0.94× | ✅ |
| https | 108,113 | 108,810 | 0.9936× | 1.0× | ✅ |
| wss | 103,737 | 104,797 | 0.9899× | 0.99× | ✅ |
| atomic | 49,719 | 51,316 | 0.9689× | 0.97× | ✅ |
| process-mode | 15,035 | 16,959 | 0.8866× | 0.88× | ✅ |
| http2-concurrent | 365,335 | 409,072 | 0.8931× | 0.88× | ✅ |
| websocket-64k | 89,859 | 159,699 | 0.5627× | 0.57× | ✅ |
| websocket-fragmented | 11,772 | 16,327 | 0.7210× | 0.73× | ✅ |
| upload | 413,277 | 1,322,387 | 0.3125× | 0.32× | ✅ |
| static | 27,418 | 41,080 | 0.6674× | 0.66× | ✅ |
| client | 18,501 | 19,757 | 0.9364× | 0.94× | ✅ |
| socket | 19,650 | 21,072 | 0.9325× | 0.92× | ✅ |
| pgsql | 23,342 | 24,595 | 0.9491× | 0.95× | ✅ |
| sqlite | 23,543 | 25,236 | 0.9329× | 0.93× | ✅ |
| curl | 85,726 | 90,475 | 0.9475× | 0.94× | ✅ |
| dns | 231,030 | 233,045 | 0.9914× | 0.99× | ✅ |

Geometric mean 0.8553×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
