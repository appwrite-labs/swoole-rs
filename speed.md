# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7434bbc` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,940 | 15,214 | 0.9163× | 0.92× | ✅ |
| echo-1k | 17,049 | 18,166 | 0.9385× | 0.94× | ✅ |
| json | 22,119 | 23,161 | 0.9550× | 0.95× | ✅ |
| service-health | 16,569 | 17,777 | 0.9321× | 0.93× | ✅ |
| websocket | 6,555 | 8,188 | 0.8006× | 0.8× | ✅ |
| timer | 204,731 | 313,778 | 0.6525× | 0.65× | ✅ |
| app | 125,640 | 135,871 | 0.9247× | 0.93× | ✅ |
| http2 | 18,652 | 20,468 | 0.9113× | 0.91× | ✅ |
| table | 195,578 | 201,865 | 0.9689× | 0.97× | ✅ |
| channel | 138,041 | 155,174 | 0.8896× | 0.89× | ✅ |
| task | 32,010 | 33,264 | 0.9623× | 0.96× | ✅ |
| mysql | 49,660 | 51,061 | 0.9726× | 0.97× | ✅ |
| http-client | 33,859 | 35,802 | 0.9457× | 0.94× | ✅ |
| client | 18,596 | 19,757 | 0.9413× | 0.94× | ✅ |
| socket | 19,725 | 21,072 | 0.9361× | 0.92× | ✅ |
| gzip | 194,176 | 197,079 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,044 | 181,572 | 0.9861× | 0.99× | ✅ |
| tcp-length | 4,708 | 4,762 | 0.9887× | 0.99× | ✅ |
| tcp-eof | 5,201 | 10,231 | 0.5083× | 0.51× | ✅ |
| udp | 7,374 | 7,986 | 0.9234× | 0.93× | ✅ |
| connect | 17,706 | 18,707 | 0.9465× | 0.94× | ✅ |
| https | 108,202 | 108,803 | 0.9945× | 1.0× | ✅ |
| wss | 103,779 | 104,797 | 0.9903× | 0.99× | ✅ |
| atomic | 49,725 | 51,316 | 0.9690× | 0.97× | ✅ |
| process-mode | 14,997 | 16,957 | 0.8845× | 0.88× | ✅ |
| http2-concurrent | 359,478 | 409,073 | 0.8788× | 0.88× | ✅ |
| websocket-64k | 89,894 | 159,695 | 0.5629× | 0.57× | ✅ |
| websocket-fragmented | 12,016 | 16,327 | 0.7359× | 0.74× | ✅ |
| upload | 412,409 | 1,322,389 | 0.3119× | 0.32× | ✅ |
| static | 27,143 | 41,051 | 0.6612× | 0.66× | ✅ |
| pgsql | 23,381 | 24,595 | 0.9506× | 0.95× | ✅ |
| sqlite | 23,573 | 25,236 | 0.9341× | 0.93× | ✅ |
| curl | 85,750 | 90,354 | 0.9490× | 0.94× | ✅ |
| dns | 231,169 | 232,647 | 0.9936× | 0.99× | ✅ |

Geometric mean 0.8563×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
