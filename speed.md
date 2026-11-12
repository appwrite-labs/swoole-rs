# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9fadf66` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,938 | 15,213 | 0.9162× | 0.92× | ✅ |
| echo-1k | 17,041 | 18,165 | 0.9381× | 0.94× | ✅ |
| json | 22,048 | 23,158 | 0.9521× | 0.95× | ✅ |
| service-health | 16,543 | 17,777 | 0.9306× | 0.93× | ✅ |
| websocket | 6,579 | 8,191 | 0.8032× | 0.82× | ✅ |
| timer | 203,364 | 313,540 | 0.6486× | 0.65× | ✅ |
| app | 125,822 | 135,906 | 0.9258× | 0.93× | ✅ |
| http2 | 18,667 | 20,464 | 0.9122× | 0.93× | ✅ |
| table | 195,746 | 201,869 | 0.9697× | 0.97× | ✅ |
| channel | 138,897 | 154,651 | 0.8981× | 0.9× | ✅ |
| task | 32,142 | 33,264 | 0.9663× | 0.96× | ✅ |
| mysql | 49,619 | 51,061 | 0.9718× | 0.97× | ✅ |
| http-client | 33,719 | 35,803 | 0.9418× | 0.94× | ✅ |
| client | 18,492 | 19,756 | 0.9360× | 0.94× | ✅ |
| socket | 19,614 | 21,068 | 0.9310× | 0.92× | ✅ |
| gzip | 196,024 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,339 | 181,568 | 0.9987× | 1.0× | ✅ |
| tcp-length | 4,773 | 4,765 | 1.0015× | 1.05× | ➖ |
| tcp-eof | 5,321 | 10,228 | 0.5202× | 0.54× | ✅ |
| udp | 7,567 | 7,987 | 0.9474× | 0.95× | ✅ |
| connect | 18,371 | 18,707 | 0.9821× | 0.98× | ✅ |
| https | 108,804 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,469 | 104,797 | 0.9969× | 1.0× | ✅ |
| atomic | 49,697 | 51,316 | 0.9685× | 0.97× | ✅ |
| process-mode | 16,719 | 17,024 | 0.9820× | 0.99× | ✅ |
| http2-concurrent | 359,494 | 408,877 | 0.8792× | 0.88× | ✅ |
| websocket-64k | 156,018 | 159,694 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,810 | 16,331 | 0.7844× | 0.79× | ✅ |
| upload | 478,434 | 1,322,382 | 0.3618× | 0.37× | ✅ |
| static | 29,181 | 40,981 | 0.7121× | 0.72× | ✅ |
| pgsql | 23,223 | 24,595 | 0.9442× | 0.95× | ✅ |
| sqlite | 23,500 | 25,236 | 0.9312× | 0.94× | ✅ |
| curl | 84,605 | 90,354 | 0.9364× | 0.94× | ✅ |
| dns | 230,626 | 232,961 | 0.9900× | 0.99× | ✅ |

Geometric mean 0.8830×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
