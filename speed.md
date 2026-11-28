# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,214 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,754 | 18,165 | 0.9223× | 0.92× | ✅ |
| json | 22,050 | 23,164 | 0.9519× | 0.95× | ✅ |
| service-health | 16,305 | 17,777 | 0.9172× | 0.92× | ✅ |
| websocket | 6,736 | 8,189 | 0.8226× | 0.81× | ✅ |
| timer | 202,186 | 313,762 | 0.6444× | 0.65× | ✅ |
| app | 127,559 | 135,983 | 0.9381× | 0.93× | ✅ |
| http2 | 19,177 | 20,496 | 0.9356× | 0.92× | ✅ |
| table | 194,345 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,019 | 155,714 | 0.8799× | 0.88× | ✅ |
| task | 32,073 | 33,272 | 0.9639× | 0.96× | ✅ |
| mysql | 50,294 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,653 | 35,796 | 0.9401× | 0.93× | ✅ |
| gzip | 194,294 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,025 | 181,573 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9537× | 0.95× | ✅ |
| connect | 17,495 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,813 | 0.9768× | 0.98× | ✅ |
| wss | 101,561 | 104,800 | 0.9691× | 0.97× | ✅ |
| atomic | 50,423 | 51,276 | 0.9834× | 0.97× | ✅ |
| process-mode | 15,404 | 17,027 | 0.9046× | 0.89× | ✅ |
| http2-concurrent | 366,814 | 408,864 | 0.8972× | 0.89× | ✅ |
| websocket-64k | 90,038 | 159,695 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,966 | 16,372 | 0.7309× | 0.72× | ✅ |
| upload | 418,897 | 1,322,425 | 0.3168× | 0.32× | ✅ |
| static | 25,393 | 41,028 | 0.6189× | 0.62× | ✅ |
| client | 18,598 | 19,720 | 0.9431× | 0.93× | ✅ |
| socket | 19,835 | 21,032 | 0.9431× | 0.93× | ✅ |
| pgsql | 22,888 | 24,658 | 0.9282× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,077 | 90,546 | 0.9506× | 0.95× | ✅ |
| dns | 229,904 | 233,033 | 0.9866× | 0.99× | ✅ |
| file | 108,880 | 131,297 | 0.8293× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
