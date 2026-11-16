# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `86e405b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,796 | 15,214 | 0.9068× | 0.92× | ✅ |
| echo-1k | 16,830 | 18,165 | 0.9265× | 0.94× | ✅ |
| json | 21,940 | 23,161 | 0.9473× | 0.95× | ✅ |
| service-health | 16,384 | 17,777 | 0.9216× | 0.93× | ✅ |
| websocket | 6,547 | 8,184 | 0.7999× | 0.8× | ✅ |
| timer | 206,312 | 313,987 | 0.6571× | 0.65× | ✅ |
| app | 125,253 | 135,863 | 0.9219× | 0.93× | ✅ |
| http2 | 18,881 | 20,464 | 0.9226× | 0.91× | ✅ |
| table | 194,985 | 201,865 | 0.9659× | 0.97× | ✅ |
| channel | 137,130 | 155,170 | 0.8837× | 0.88× | ✅ |
| task | 32,320 | 33,264 | 0.9716× | 0.96× | ✅ |
| mysql | 49,427 | 51,061 | 0.9680× | 0.97× | ✅ |
| http-client | 33,014 | 35,800 | 0.9222× | 0.94× | ✅ |
| gzip | 193,945 | 197,079 | 0.9841× | 0.99× | ✅ |
| websocket-deflate | 178,870 | 181,568 | 0.9851× | 0.99× | ✅ |
| tcp-length | 4,650 | 4,762 | 0.9765× | 0.99× | ✅ |
| tcp-eof | 5,145 | 10,231 | 0.5029× | 0.51× | ✅ |
| udp | 7,482 | 7,987 | 0.9368× | 0.93× | ✅ |
| connect | 17,565 | 18,707 | 0.9390× | 0.94× | ✅ |
| https | 108,051 | 108,806 | 0.9931× | 1.0× | ✅ |
| wss | 103,732 | 104,794 | 0.9899× | 0.99× | ✅ |
| atomic | 49,648 | 51,316 | 0.9675× | 0.97× | ✅ |
| process-mode | 14,954 | 16,959 | 0.8818× | 0.88× | ✅ |
| http2-concurrent | 365,276 | 409,072 | 0.8929× | 0.88× | ✅ |
| websocket-64k | 89,833 | 159,698 | 0.5625× | 0.57× | ✅ |
| websocket-fragmented | 11,740 | 16,327 | 0.7190× | 0.73× | ✅ |
| upload | 413,790 | 1,322,390 | 0.3129× | 0.32× | ✅ |
| static | 27,170 | 41,080 | 0.6614× | 0.66× | ✅ |
| client | 18,431 | 19,756 | 0.9329× | 0.94× | ✅ |
| socket | 19,584 | 21,072 | 0.9294× | 0.92× | ✅ |
| pgsql | 23,234 | 24,594 | 0.9447× | 0.95× | ✅ |
| sqlite | 23,448 | 25,236 | 0.9291× | 0.93× | ✅ |
| curl | 85,630 | 90,476 | 0.9464× | 0.94× | ✅ |
| dns | 230,600 | 232,788 | 0.9906× | 0.99× | ✅ |

Geometric mean 0.8533×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
