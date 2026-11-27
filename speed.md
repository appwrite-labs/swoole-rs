# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,214 | 0.9017× | 0.91× | ✅ |
| echo-1k | 16,754 | 18,165 | 0.9223× | 0.92× | ✅ |
| json | 22,047 | 23,174 | 0.9514× | 0.95× | ✅ |
| service-health | 16,305 | 17,786 | 0.9167× | 0.92× | ✅ |
| websocket | 6,740 | 8,189 | 0.8231× | 0.81× | ✅ |
| timer | 202,182 | 313,849 | 0.6442× | 0.65× | ✅ |
| app | 127,561 | 135,989 | 0.9380× | 0.93× | ✅ |
| http2 | 19,176 | 20,496 | 0.9356× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,019 | 155,723 | 0.8799× | 0.88× | ✅ |
| task | 32,069 | 33,269 | 0.9639× | 0.96× | ✅ |
| mysql | 50,294 | 51,085 | 0.9845× | 0.97× | ✅ |
| http-client | 33,653 | 35,796 | 0.9402× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,570 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,495 | 18,730 | 0.9340× | 0.93× | ✅ |
| https | 106,288 | 108,813 | 0.9768× | 0.98× | ✅ |
| wss | 101,572 | 104,806 | 0.9691× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,404 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 367,088 | 409,053 | 0.8974× | 0.89× | ✅ |
| websocket-64k | 90,033 | 159,691 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,368 | 0.7309× | 0.72× | ✅ |
| upload | 418,892 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,364 | 40,999 | 0.6186× | 0.62× | ✅ |
| client | 18,596 | 19,730 | 0.9425× | 0.93× | ✅ |
| socket | 19,836 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,892 | 24,666 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,073 | 90,659 | 0.9494× | 0.95× | ✅ |
| dns | 229,670 | 232,876 | 0.9862× | 0.99× | ✅ |
| file | 108,880 | 131,300 | 0.8292× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
