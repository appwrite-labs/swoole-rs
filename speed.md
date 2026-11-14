# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `69f75e4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,941 | 15,214 | 0.9163× | 0.92× | ✅ |
| echo-1k | 17,053 | 18,163 | 0.9389× | 0.94× | ✅ |
| json | 22,110 | 23,161 | 0.9546× | 0.95× | ✅ |
| service-health | 16,555 | 17,777 | 0.9313× | 0.93× | ✅ |
| websocket | 6,564 | 8,187 | 0.8018× | 0.8× | ✅ |
| timer | 207,082 | 313,576 | 0.6604× | 0.65× | ✅ |
| app | 125,629 | 135,924 | 0.9243× | 0.93× | ✅ |
| http2 | 18,671 | 20,468 | 0.9122× | 0.91× | ✅ |
| table | 196,013 | 201,865 | 0.9710× | 0.97× | ✅ |
| channel | 138,030 | 155,170 | 0.8895× | 0.89× | ✅ |
| task | 32,365 | 33,261 | 0.9731× | 0.96× | ✅ |
| mysql | 49,685 | 51,061 | 0.9731× | 0.97× | ✅ |
| http-client | 33,913 | 35,801 | 0.9472× | 0.94× | ✅ |
| client | 18,598 | 19,756 | 0.9414× | 0.94× | ✅ |
| socket | 19,732 | 21,069 | 0.9365× | 0.92× | ✅ |
| gzip | 194,185 | 197,083 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,050 | 181,572 | 0.9861× | 0.99× | ✅ |
| tcp-length | 4,714 | 4,762 | 0.9899× | 0.99× | ✅ |
| tcp-eof | 5,207 | 10,231 | 0.5089× | 0.51× | ✅ |
| udp | 7,388 | 7,987 | 0.9251× | 0.93× | ✅ |
| connect | 17,684 | 18,707 | 0.9453× | 0.94× | ✅ |
| https | 108,216 | 108,806 | 0.9946× | 1.0× | ✅ |
| wss | 103,803 | 104,794 | 0.9905× | 0.99× | ✅ |
| atomic | 49,712 | 51,312 | 0.9688× | 0.97× | ✅ |
| process-mode | 15,083 | 16,957 | 0.8895× | 0.88× | ✅ |
| http2-concurrent | 359,220 | 409,789 | 0.8766× | 0.88× | ✅ |
| websocket-64k | 89,898 | 159,696 | 0.5629× | 0.57× | ✅ |
| websocket-fragmented | 12,025 | 16,327 | 0.7365× | 0.74× | ✅ |
| upload | 412,404 | 1,322,387 | 0.3119× | 0.32× | ✅ |
| static | 27,151 | 41,051 | 0.6614× | 0.66× | ✅ |
| pgsql | 23,348 | 24,595 | 0.9493× | 0.95× | ✅ |
| sqlite | 23,560 | 25,236 | 0.9336× | 0.93× | ✅ |
| curl | 85,757 | 90,350 | 0.9492× | 0.94× | ✅ |
| dns | 231,617 | 233,246 | 0.9930× | 0.99× | ✅ |

Geometric mean 0.8572×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
