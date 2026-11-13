# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `840211e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,825 | 15,213 | 0.9088× | 0.92× | ✅ |
| echo-1k | 16,926 | 18,165 | 0.9318× | 0.94× | ✅ |
| json | 21,928 | 23,161 | 0.9468× | 0.95× | ✅ |
| service-health | 16,416 | 17,777 | 0.9235× | 0.93× | ✅ |
| websocket | 6,477 | 8,185 | 0.7914× | 0.8× | ✅ |
| timer | 203,141 | 313,875 | 0.6472× | 0.65× | ✅ |
| app | 125,271 | 135,885 | 0.9219× | 0.93× | ✅ |
| http2 | 18,489 | 20,468 | 0.9033× | 0.91× | ✅ |
| table | 195,433 | 201,865 | 0.9681× | 0.97× | ✅ |
| channel | 137,378 | 155,170 | 0.8853× | 0.89× | ✅ |
| task | 31,875 | 33,263 | 0.9583× | 0.96× | ✅ |
| mysql | 49,420 | 51,061 | 0.9679× | 0.97× | ✅ |
| http-client | 33,649 | 35,803 | 0.9398× | 0.94× | ✅ |
| client | 18,414 | 19,757 | 0.9320× | 0.94× | ✅ |
| socket | 19,539 | 21,072 | 0.9272× | 0.92× | ✅ |
| gzip | 194,032 | 197,079 | 0.9845× | 0.99× | ✅ |
| websocket-deflate | 179,301 | 181,572 | 0.9875× | 0.99× | ✅ |
| tcp-length | 4,692 | 4,762 | 0.9853× | 0.99× | ✅ |
| tcp-eof | 5,215 | 10,231 | 0.5097× | 0.51× | ✅ |
| udp | 7,378 | 7,987 | 0.9238× | 0.93× | ✅ |
| connect | 17,491 | 18,703 | 0.9352× | 0.94× | ✅ |
| https | 108,049 | 108,810 | 0.9930× | 1.0× | ✅ |
| wss | 103,624 | 104,801 | 0.9888× | 0.99× | ✅ |
| atomic | 49,569 | 51,316 | 0.9660× | 0.97× | ✅ |
| process-mode | 14,905 | 16,963 | 0.8787× | 0.88× | ✅ |
| http2-concurrent | 356,979 | 408,877 | 0.8731× | 0.88× | ✅ |
| websocket-64k | 155,928 | 159,691 | 0.9764× | 0.98× | ✅ |
| websocket-fragmented | 12,706 | 16,331 | 0.7781× | 0.78× | ✅ |
| upload | 478,023 | 1,322,385 | 0.3615× | 0.37× | ✅ |
| static | 27,182 | 41,010 | 0.6628× | 0.67× | ✅ |
| pgsql | 23,151 | 24,595 | 0.9413× | 0.95× | ✅ |
| sqlite | 23,414 | 25,236 | 0.9278× | 0.93× | ✅ |
| curl | 85,656 | 90,469 | 0.9468× | 0.94× | ✅ |
| dns | 230,815 | 232,681 | 0.9920× | 0.99× | ✅ |

Geometric mean 0.8716×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
