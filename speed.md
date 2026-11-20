# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `97ce799` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,768 | 15,214 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,810 | 18,165 | 0.9254× | 0.92× | ✅ |
| json | 22,093 | 23,164 | 0.9538× | 0.95× | ✅ |
| service-health | 16,351 | 17,777 | 0.9198× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,995 | 313,937 | 0.6434× | 0.65× | ✅ |
| app | 127,630 | 135,967 | 0.9387× | 0.93× | ✅ |
| http2 | 18,971 | 20,500 | 0.9254× | 0.92× | ✅ |
| table | 194,387 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,375 | 155,717 | 0.8822× | 0.88× | ✅ |
| task | 32,088 | 33,269 | 0.9645× | 0.96× | ✅ |
| mysql | 50,325 | 51,085 | 0.9851× | 0.97× | ✅ |
| http-client | 33,678 | 35,796 | 0.9408× | 0.93× | ✅ |
| gzip | 194,296 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,545 | 18,730 | 0.9367× | 0.93× | ✅ |
| https | 106,334 | 108,809 | 0.9773× | 0.98× | ✅ |
| wss | 101,597 | 104,803 | 0.9694× | 0.97× | ✅ |
| atomic | 50,217 | 51,285 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,438 | 17,021 | 0.9070× | 0.89× | ✅ |
| http2-concurrent | 363,522 | 409,053 | 0.8887× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,525 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,937 | 41,063 | 0.6316× | 0.64× | ✅ |
| client | 18,642 | 19,730 | 0.9449× | 0.93× | ✅ |
| socket | 19,881 | 21,045 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,827 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,488 | 25,268 | 0.9296× | 0.93× | ✅ |
| curl | 86,142 | 90,418 | 0.9527× | 0.95× | ✅ |
| dns | 230,296 | 232,893 | 0.9888× | 0.99× | ✅ |
| file | 109,092 | 131,208 | 0.8314× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
