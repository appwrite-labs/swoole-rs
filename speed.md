# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `db41500` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,870 | 15,210 | 0.9119× | 0.92× | ✅ |
| echo-1k | 16,970 | 18,165 | 0.9342× | 0.94× | ✅ |
| json | 21,962 | 23,161 | 0.9482× | 0.95× | ✅ |
| service-health | 16,454 | 17,774 | 0.9257× | 0.93× | ✅ |
| websocket | 6,476 | 8,188 | 0.7909× | 0.8× | ✅ |
| timer | 203,042 | 313,609 | 0.6474× | 0.65× | ✅ |
| app | 125,339 | 135,897 | 0.9223× | 0.93× | ✅ |
| http2 | 18,489 | 20,467 | 0.9033× | 0.91× | ✅ |
| table | 195,428 | 201,865 | 0.9681× | 0.97× | ✅ |
| channel | 137,366 | 155,170 | 0.8853× | 0.89× | ✅ |
| task | 31,933 | 33,261 | 0.9601× | 0.96× | ✅ |
| mysql | 49,466 | 51,064 | 0.9687× | 0.97× | ✅ |
| http-client | 33,629 | 35,795 | 0.9395× | 0.94× | ✅ |
| client | 18,406 | 19,757 | 0.9316× | 0.94× | ✅ |
| socket | 19,532 | 21,072 | 0.9269× | 0.92× | ✅ |
| gzip | 194,070 | 197,083 | 0.9847× | 0.99× | ✅ |
| websocket-deflate | 179,309 | 181,572 | 0.9875× | 0.99× | ✅ |
| tcp-length | 4,690 | 4,762 | 0.9849× | 0.99× | ✅ |
| tcp-eof | 5,213 | 10,228 | 0.5097× | 0.51× | ✅ |
| udp | 7,378 | 7,986 | 0.9238× | 0.93× | ✅ |
| connect | 17,540 | 18,707 | 0.9376× | 0.95× | ✅ |
| https | 108,028 | 108,814 | 0.9928× | 1.0× | ✅ |
| wss | 103,623 | 104,790 | 0.9889× | 0.99× | ✅ |
| atomic | 49,585 | 51,316 | 0.9663× | 0.97× | ✅ |
| process-mode | 14,900 | 17,027 | 0.8751× | 0.89× | ✅ |
| http2-concurrent | 357,488 | 408,881 | 0.8743× | 0.88× | ✅ |
| websocket-64k | 155,918 | 159,693 | 0.9764× | 0.98× | ✅ |
| websocket-fragmented | 12,704 | 16,328 | 0.7781× | 0.78× | ✅ |
| upload | 477,818 | 1,322,384 | 0.3613× | 0.37× | ✅ |
| static | 27,266 | 40,981 | 0.6653× | 0.67× | ✅ |
| pgsql | 23,146 | 24,596 | 0.9410× | 0.95× | ✅ |
| sqlite | 23,411 | 25,236 | 0.9277× | 0.93× | ✅ |
| curl | 85,643 | 90,353 | 0.9479× | 0.94× | ✅ |
| dns | 231,123 | 232,895 | 0.9924× | 0.99× | ✅ |

Geometric mean 0.8720×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
