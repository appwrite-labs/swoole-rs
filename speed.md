# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4accc8f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,774 | 15,214 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,788 | 18,165 | 0.9242× | 0.93× | ✅ |
| json | 21,938 | 23,161 | 0.9472× | 0.95× | ✅ |
| service-health | 16,351 | 17,777 | 0.9198× | 0.92× | ✅ |
| websocket | 6,528 | 8,188 | 0.7973× | 0.8× | ✅ |
| timer | 206,182 | 313,571 | 0.6575× | 0.65× | ✅ |
| app | 126,245 | 135,927 | 0.9288× | 0.93× | ✅ |
| http2 | 19,097 | 20,469 | 0.9330× | 0.92× | ✅ |
| table | 194,355 | 201,865 | 0.9628× | 0.97× | ✅ |
| channel | 137,566 | 155,174 | 0.8865× | 0.88× | ✅ |
| task | 32,491 | 33,268 | 0.9766× | 0.96× | ✅ |
| mysql | 49,647 | 51,053 | 0.9725× | 0.97× | ✅ |
| http-client | 33,368 | 35,800 | 0.9321× | 0.93× | ✅ |
| gzip | 194,443 | 197,079 | 0.9866× | 0.99× | ✅ |
| websocket-deflate | 178,981 | 181,572 | 0.9857× | 0.99× | ✅ |
| tcp-length | 4,564 | 4,765 | 0.9577× | 0.96× | ✅ |
| tcp-eof | 5,144 | 10,231 | 0.5027× | 0.5× | ✅ |
| udp | 7,463 | 7,986 | 0.9345× | 0.93× | ✅ |
| connect | 17,406 | 18,707 | 0.9304× | 0.93× | ✅ |
| https | 105,913 | 108,810 | 0.9734× | 0.98× | ✅ |
| wss | 100,947 | 104,794 | 0.9633× | 0.97× | ✅ |
| atomic | 49,846 | 51,312 | 0.9714× | 0.97× | ✅ |
| process-mode | 14,985 | 16,959 | 0.8836× | 0.88× | ✅ |
| http2-concurrent | 365,890 | 409,072 | 0.8944× | 0.88× | ✅ |
| websocket-64k | 89,786 | 159,696 | 0.5622× | 0.57× | ✅ |
| websocket-fragmented | 11,598 | 16,331 | 0.7102× | 0.72× | ✅ |
| upload | 415,225 | 1,322,387 | 0.3140× | 0.32× | ✅ |
| static | 26,532 | 40,987 | 0.6473× | 0.65× | ✅ |
| client | 18,418 | 19,753 | 0.9324× | 0.94× | ✅ |
| socket | 19,585 | 21,073 | 0.9294× | 0.92× | ✅ |
| pgsql | 22,658 | 24,594 | 0.9213× | 0.93× | ✅ |
| sqlite | 23,408 | 25,236 | 0.9275× | 0.93× | ✅ |
| curl | 86,151 | 90,350 | 0.9535× | 0.94× | ✅ |
| dns | 230,256 | 232,649 | 0.9897× | 0.99× | ✅ |

Geometric mean 0.8511×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
