# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `97ce799` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,214 | 0.9052× | 0.91× | ✅ |
| echo-1k | 16,810 | 18,165 | 0.9254× | 0.92× | ✅ |
| json | 22,093 | 23,164 | 0.9537× | 0.95× | ✅ |
| service-health | 16,351 | 17,777 | 0.9198× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 202,035 | 313,809 | 0.6438× | 0.65× | ✅ |
| app | 127,657 | 135,940 | 0.9391× | 0.93× | ✅ |
| http2 | 18,969 | 20,500 | 0.9253× | 0.92× | ✅ |
| table | 194,387 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,375 | 155,717 | 0.8822× | 0.88× | ✅ |
| task | 32,089 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,646 | 35,792 | 0.9400× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9785× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,542 | 18,734 | 0.9364× | 0.93× | ✅ |
| https | 106,331 | 108,802 | 0.9773× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,214 | 51,285 | 0.9791× | 0.97× | ✅ |
| process-mode | 15,434 | 17,027 | 0.9064× | 0.89× | ✅ |
| http2-concurrent | 363,605 | 409,057 | 0.8889× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,695 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,529 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,937 | 41,063 | 0.6316× | 0.64× | ✅ |
| client | 18,642 | 19,730 | 0.9448× | 0.93× | ✅ |
| socket | 19,878 | 21,045 | 0.9445× | 0.93× | ✅ |
| pgsql | 22,831 | 24,662 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 86,136 | 90,417 | 0.9527× | 0.95× | ✅ |
| dns | 229,934 | 233,083 | 0.9865× | 0.99× | ✅ |
| file | 109,092 | 131,211 | 0.8314× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
