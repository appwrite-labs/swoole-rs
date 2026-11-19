# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f2e2834` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,744 | 15,213 | 0.9034× | 0.91× | ✅ |
| echo-1k | 16,760 | 18,162 | 0.9228× | 0.92× | ✅ |
| json | 22,068 | 23,161 | 0.9528× | 0.95× | ✅ |
| service-health | 16,334 | 17,773 | 0.9190× | 0.92× | ✅ |
| websocket | 6,676 | 8,189 | 0.8152× | 0.81× | ✅ |
| timer | 201,534 | 313,808 | 0.6422× | 0.65× | ✅ |
| app | 126,974 | 135,933 | 0.9341× | 0.93× | ✅ |
| http2 | 18,965 | 20,496 | 0.9253× | 0.92× | ✅ |
| table | 193,853 | 202,900 | 0.9554× | 0.96× | ✅ |
| channel | 137,353 | 155,714 | 0.8821× | 0.88× | ✅ |
| task | 31,957 | 33,269 | 0.9606× | 0.96× | ✅ |
| mysql | 50,042 | 51,088 | 0.9795× | 0.97× | ✅ |
| http-client | 33,365 | 35,784 | 0.9324× | 0.93× | ✅ |
| gzip | 194,378 | 197,095 | 0.9862× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,562 | 0.9865× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,766 | 0.9748× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,560 | 7,986 | 0.9467× | 0.95× | ✅ |
| connect | 17,580 | 18,734 | 0.9384× | 0.93× | ✅ |
| https | 106,004 | 108,809 | 0.9742× | 0.98× | ✅ |
| wss | 101,216 | 104,797 | 0.9658× | 0.97× | ✅ |
| atomic | 50,184 | 51,285 | 0.9785× | 0.97× | ✅ |
| process-mode | 15,267 | 17,025 | 0.8968× | 0.89× | ✅ |
| http2-concurrent | 365,048 | 409,397 | 0.8917× | 0.89× | ✅ |
| websocket-64k | 89,974 | 159,694 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,839 | 16,372 | 0.7231× | 0.72× | ✅ |
| upload | 416,435 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,057 | 41,005 | 0.6598× | 0.65× | ✅ |
| client | 18,476 | 19,730 | 0.9364× | 0.93× | ✅ |
| socket | 19,704 | 21,045 | 0.9363× | 0.93× | ✅ |
| pgsql | 23,091 | 24,663 | 0.9363× | 0.92× | ✅ |
| sqlite | 23,433 | 25,268 | 0.9274× | 0.93× | ✅ |
| curl | 85,924 | 90,421 | 0.9503× | 0.95× | ✅ |
| dns | 230,160 | 233,226 | 0.9869× | 0.99× | ✅ |
| file | 108,206 | 131,300 | 0.8241× | 0.83× | ✅ |

Geometric mean 0.8530×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
