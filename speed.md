# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a8204ba` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,812 | 15,214 | 0.9079× | 0.92× | ✅ |
| echo-1k | 16,909 | 18,165 | 0.9309× | 0.94× | ✅ |
| json | 21,913 | 23,161 | 0.9461× | 0.95× | ✅ |
| service-health | 16,413 | 17,777 | 0.9233× | 0.93× | ✅ |
| websocket | 6,465 | 8,188 | 0.7897× | 0.8× | ✅ |
| timer | 203,167 | 313,659 | 0.6477× | 0.65× | ✅ |
| app | 125,298 | 135,957 | 0.9216× | 0.93× | ✅ |
| http2 | 18,489 | 20,468 | 0.9033× | 0.91× | ✅ |
| table | 195,498 | 201,865 | 0.9685× | 0.97× | ✅ |
| channel | 137,377 | 155,170 | 0.8853× | 0.89× | ✅ |
| task | 31,866 | 33,264 | 0.9580× | 0.96× | ✅ |
| mysql | 49,465 | 51,037 | 0.9692× | 0.97× | ✅ |
| http-client | 33,683 | 35,803 | 0.9408× | 0.94× | ✅ |
| client | 18,429 | 19,757 | 0.9328× | 0.94× | ✅ |
| socket | 19,557 | 21,073 | 0.9281× | 0.92× | ✅ |
| gzip | 194,020 | 197,079 | 0.9845× | 0.99× | ✅ |
| websocket-deflate | 179,286 | 181,568 | 0.9874× | 0.99× | ✅ |
| tcp-length | 4,681 | 4,762 | 0.9830× | 0.99× | ✅ |
| tcp-eof | 5,204 | 10,228 | 0.5088× | 0.51× | ✅ |
| udp | 7,370 | 7,987 | 0.9228× | 0.93× | ✅ |
| connect | 17,516 | 18,707 | 0.9364× | 0.94× | ✅ |
| https | 108,054 | 108,803 | 0.9931× | 1.0× | ✅ |
| wss | 103,685 | 104,800 | 0.9894× | 0.99× | ✅ |
| atomic | 49,577 | 51,312 | 0.9662× | 0.97× | ✅ |
| process-mode | 14,900 | 16,959 | 0.8786× | 0.88× | ✅ |
| http2-concurrent | 356,866 | 409,069 | 0.8724× | 0.88× | ✅ |
| websocket-64k | 155,912 | 159,692 | 0.9763× | 0.98× | ✅ |
| websocket-fragmented | 12,694 | 16,331 | 0.7773× | 0.78× | ✅ |
| upload | 478,001 | 1,322,388 | 0.3615× | 0.37× | ✅ |
| static | 26,953 | 40,987 | 0.6576× | 0.67× | ✅ |
| pgsql | 23,188 | 24,595 | 0.9428× | 0.95× | ✅ |
| sqlite | 23,396 | 25,236 | 0.9271× | 0.93× | ✅ |
| curl | 85,745 | 90,349 | 0.9490× | 0.94× | ✅ |
| dns | 230,531 | 232,439 | 0.9918× | 0.99× | ✅ |

Geometric mean 0.8713×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
