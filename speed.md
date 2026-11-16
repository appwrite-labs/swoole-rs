# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ef20258` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,934 | 15,213 | 0.9159× | 0.92× | ✅ |
| echo-1k | 17,029 | 18,165 | 0.9374× | 0.94× | ✅ |
| json | 22,089 | 23,161 | 0.9537× | 0.95× | ✅ |
| service-health | 16,529 | 17,777 | 0.9298× | 0.93× | ✅ |
| websocket | 6,584 | 8,188 | 0.8041× | 0.8× | ✅ |
| timer | 206,087 | 313,752 | 0.6568× | 0.65× | ✅ |
| app | 125,434 | 135,895 | 0.9230× | 0.93× | ✅ |
| http2 | 18,905 | 20,468 | 0.9236× | 0.91× | ✅ |
| table | 195,073 | 201,865 | 0.9664× | 0.97× | ✅ |
| channel | 136,705 | 155,170 | 0.8810× | 0.88× | ✅ |
| task | 32,517 | 33,265 | 0.9775× | 0.96× | ✅ |
| mysql | 49,556 | 51,061 | 0.9705× | 0.97× | ✅ |
| http-client | 33,900 | 35,800 | 0.9469× | 0.94× | ✅ |
| gzip | 194,144 | 197,079 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,016 | 181,560 | 0.9860× | 0.99× | ✅ |
| tcp-length | 4,671 | 4,762 | 0.9809× | 0.99× | ✅ |
| tcp-eof | 5,166 | 10,231 | 0.5049× | 0.51× | ✅ |
| udp | 7,450 | 7,987 | 0.9328× | 0.93× | ✅ |
| connect | 17,638 | 18,707 | 0.9429× | 0.94× | ✅ |
| https | 108,160 | 108,803 | 0.9941× | 1.0× | ✅ |
| wss | 103,786 | 104,794 | 0.9904× | 0.99× | ✅ |
| atomic | 49,740 | 51,316 | 0.9693× | 0.97× | ✅ |
| process-mode | 15,060 | 16,960 | 0.8880× | 0.88× | ✅ |
| http2-concurrent | 366,453 | 409,069 | 0.8958× | 0.88× | ✅ |
| websocket-64k | 89,906 | 159,695 | 0.5630× | 0.57× | ✅ |
| websocket-fragmented | 11,833 | 16,327 | 0.7247× | 0.73× | ✅ |
| upload | 413,492 | 1,322,389 | 0.3127× | 0.32× | ✅ |
| static | 27,448 | 41,080 | 0.6682× | 0.66× | ✅ |
| client | 18,522 | 19,757 | 0.9375× | 0.94× | ✅ |
| socket | 19,665 | 21,072 | 0.9332× | 0.92× | ✅ |
| pgsql | 23,342 | 24,595 | 0.9490× | 0.95× | ✅ |
| sqlite | 23,553 | 25,221 | 0.9339× | 0.93× | ✅ |
| curl | 86,112 | 90,313 | 0.9535× | 0.94× | ✅ |
| dns | 231,190 | 232,924 | 0.9926× | 0.99× | ✅ |

Geometric mean 0.8570×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
