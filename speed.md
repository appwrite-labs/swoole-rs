# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `fc87bd8` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,386 | 15,206 | 0.9461× | 0.91× | ✅ |
| echo-1k | 17,353 | 18,174 | 0.9549× | 0.92× | ✅ |
| json | 22,401 | 23,173 | 0.9667× | 0.94× | ✅ |
| service-health | 16,958 | 17,783 | 0.9536× | 0.93× | ✅ |
| websocket | 6,565 | 8,188 | 0.8018× | 0.81× | ✅ |
| timer | 209,272 | 313,433 | 0.6677× | 0.68× | ✅ |
| app | 125,853 | 136,032 | 0.9252× | 0.92× | ✅ |
| http2 | 19,793 | 20,488 | 0.9661× | 0.95× | ✅ |
| table | 197,916 | 201,934 | 0.9801× | 0.98× | ✅ |
| channel | 139,776 | 155,262 | 0.9003× | 0.89× | ✅ |
| task | 31,871 | 33,315 | 0.9567× | 0.94× | ✅ |
| mysql | 49,566 | 50,983 | 0.9722× | 0.97× | ✅ |
| http-client | 33,723 | 35,828 | 0.9413× | 0.97× | ✅ |
| client | 18,505 | 19,548 | 0.9466× | 0.96× | ✅ |
| socket | 18,125 | 19,330 | 0.9377× | 0.99× | ✅ |
| gzip | 200,789 | 197,094 | 1.0187× | 1.04× | ➖ |
| websocket-deflate | 181,365 | 181,568 | 0.9989× | 1.04× | ✅ |
| tcp-length | 5,054 | 4,746 | 1.0649× | 1.02× | ❌ |
| tcp-eof | 5,592 | 10,212 | 0.5476× | 0.57× | ✅ |
| udp | 7,834 | 7,359 | 1.0646× | 1.09× | ➖ |
| https | 109,146 | 108,807 | 1.0031× | 1.04× | ➖ |
| wss | 104,245 | 104,802 | 0.9947× | 1.04× | ✅ |
| atomic | 52,348 | 51,334 | 1.0198× | 1.02× | ➖ |
| process-mode | 16,510 | 16,946 | 0.9743× | 0.95× | ✅ |

Geometric mean 0.9291×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
