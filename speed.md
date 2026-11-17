# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `62c8388` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,782 | 15,213 | 0.9059× | 0.91× | ✅ |
| echo-1k | 16,799 | 18,165 | 0.9248× | 0.93× | ✅ |
| json | 21,941 | 23,161 | 0.9473× | 0.95× | ✅ |
| service-health | 16,354 | 17,777 | 0.9200× | 0.92× | ✅ |
| websocket | 6,538 | 8,191 | 0.7981× | 0.8× | ✅ |
| timer | 206,131 | 313,572 | 0.6574× | 0.65× | ✅ |
| app | 126,443 | 135,877 | 0.9306× | 0.93× | ✅ |
| http2 | 19,102 | 20,467 | 0.9333× | 0.92× | ✅ |
| table | 194,806 | 201,865 | 0.9650× | 0.97× | ✅ |
| channel | 137,572 | 155,170 | 0.8866× | 0.88× | ✅ |
| task | 32,490 | 33,268 | 0.9766× | 0.96× | ✅ |
| mysql | 49,780 | 51,064 | 0.9748× | 0.97× | ✅ |
| http-client | 33,471 | 35,799 | 0.9350× | 0.93× | ✅ |
| gzip | 194,454 | 197,079 | 0.9867× | 0.99× | ✅ |
| websocket-deflate | 178,991 | 181,572 | 0.9858× | 0.99× | ✅ |
| tcp-length | 4,574 | 4,762 | 0.9605× | 0.96× | ✅ |
| tcp-eof | 5,154 | 10,231 | 0.5037× | 0.5× | ✅ |
| udp | 7,473 | 7,987 | 0.9357× | 0.93× | ✅ |
| connect | 17,493 | 18,707 | 0.9351× | 0.93× | ✅ |
| https | 105,936 | 108,810 | 0.9736× | 0.98× | ✅ |
| wss | 100,969 | 104,794 | 0.9635× | 0.97× | ✅ |
| atomic | 49,857 | 51,316 | 0.9716× | 0.97× | ✅ |
| process-mode | 15,004 | 16,960 | 0.8847× | 0.88× | ✅ |
| http2-concurrent | 365,758 | 409,073 | 0.8941× | 0.88× | ✅ |
| websocket-64k | 89,794 | 159,695 | 0.5623× | 0.57× | ✅ |
| websocket-fragmented | 11,609 | 16,327 | 0.7110× | 0.72× | ✅ |
| upload | 415,248 | 1,322,387 | 0.3140× | 0.32× | ✅ |
| static | 26,696 | 41,051 | 0.6503× | 0.65× | ✅ |
| client | 18,483 | 19,756 | 0.9356× | 0.94× | ✅ |
| socket | 19,642 | 21,072 | 0.9321× | 0.92× | ✅ |
| pgsql | 22,782 | 24,595 | 0.9263× | 0.93× | ✅ |
| sqlite | 23,443 | 25,236 | 0.9289× | 0.93× | ✅ |
| curl | 86,338 | 90,319 | 0.9559× | 0.94× | ✅ |
| dns | 230,422 | 233,093 | 0.9885× | 0.99× | ✅ |

Geometric mean 0.8522×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
