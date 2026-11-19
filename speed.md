# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `10ea1e1` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,747 | 15,214 | 0.9036× | 0.91× | ✅ |
| echo-1k | 16,760 | 18,162 | 0.9228× | 0.92× | ✅ |
| json | 22,077 | 23,173 | 0.9527× | 0.95× | ✅ |
| service-health | 16,334 | 17,782 | 0.9186× | 0.92× | ✅ |
| websocket | 6,671 | 8,192 | 0.8143× | 0.81× | ✅ |
| timer | 201,495 | 313,968 | 0.6418× | 0.65× | ✅ |
| app | 127,026 | 135,956 | 0.9343× | 0.93× | ✅ |
| http2 | 18,961 | 20,497 | 0.9250× | 0.92× | ✅ |
| table | 194,301 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,357 | 155,727 | 0.8820× | 0.88× | ✅ |
| task | 31,964 | 33,268 | 0.9608× | 0.96× | ✅ |
| mysql | 50,050 | 51,085 | 0.9797× | 0.97× | ✅ |
| http-client | 33,373 | 35,788 | 0.9325× | 0.93× | ✅ |
| gzip | 194,378 | 197,099 | 0.9862× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,565 | 0.9865× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,766 | 0.9748× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,561 | 7,987 | 0.9467× | 0.95× | ✅ |
| connect | 17,588 | 18,734 | 0.9388× | 0.93× | ✅ |
| https | 106,004 | 108,809 | 0.9742× | 0.98× | ✅ |
| wss | 101,216 | 104,797 | 0.9658× | 0.97× | ✅ |
| atomic | 50,184 | 51,285 | 0.9785× | 0.97× | ✅ |
| process-mode | 15,271 | 17,028 | 0.8968× | 0.89× | ✅ |
| http2-concurrent | 364,730 | 409,053 | 0.8916× | 0.89× | ✅ |
| websocket-64k | 89,969 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,839 | 16,368 | 0.7233× | 0.72× | ✅ |
| upload | 416,440 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,144 | 41,092 | 0.6606× | 0.65× | ✅ |
| client | 18,479 | 19,730 | 0.9366× | 0.93× | ✅ |
| socket | 19,705 | 21,046 | 0.9363× | 0.93× | ✅ |
| pgsql | 23,091 | 24,663 | 0.9363× | 0.92× | ✅ |
| sqlite | 23,433 | 25,268 | 0.9274× | 0.93× | ✅ |
| curl | 85,652 | 90,546 | 0.9459× | 0.95× | ✅ |
| dns | 229,901 | 233,055 | 0.9865× | 0.99× | ✅ |
| file | 108,184 | 131,207 | 0.8245× | 0.83× | ✅ |

Geometric mean 0.8531×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
