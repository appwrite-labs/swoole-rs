# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `69f4b96` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,741 | 15,214 | 0.9032× | 0.91× | ✅ |
| echo-1k | 16,724 | 18,162 | 0.9208× | 0.92× | ✅ |
| json | 22,061 | 23,173 | 0.9520× | 0.95× | ✅ |
| service-health | 16,305 | 17,786 | 0.9167× | 0.92× | ✅ |
| websocket | 6,681 | 8,186 | 0.8161× | 0.8× | ✅ |
| timer | 201,568 | 313,828 | 0.6423× | 0.65× | ✅ |
| app | 127,002 | 135,983 | 0.9340× | 0.93× | ✅ |
| http2 | 18,969 | 20,496 | 0.9255× | 0.92× | ✅ |
| table | 193,897 | 201,885 | 0.9604× | 0.96× | ✅ |
| channel | 137,963 | 155,723 | 0.8860× | 0.88× | ✅ |
| task | 31,932 | 33,268 | 0.9598× | 0.96× | ✅ |
| mysql | 50,035 | 51,062 | 0.9799× | 0.97× | ✅ |
| http-client | 33,836 | 35,797 | 0.9452× | 0.93× | ✅ |
| gzip | 194,346 | 197,095 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,763 | 0.9750× | 0.96× | ✅ |
| tcp-eof | 5,194 | 10,228 | 0.5078× | 0.5× | ✅ |
| udp | 7,579 | 7,986 | 0.9490× | 0.95× | ✅ |
| connect | 17,590 | 18,734 | 0.9390× | 0.93× | ✅ |
| https | 106,025 | 108,809 | 0.9744× | 0.98× | ✅ |
| wss | 101,243 | 104,797 | 0.9661× | 0.97× | ✅ |
| atomic | 50,167 | 51,281 | 0.9783× | 0.97× | ✅ |
| process-mode | 15,270 | 17,021 | 0.8971× | 0.89× | ✅ |
| http2-concurrent | 364,358 | 409,053 | 0.8907× | 0.89× | ✅ |
| websocket-64k | 89,981 | 159,691 | 0.5635× | 0.57× | ✅ |
| websocket-fragmented | 11,840 | 16,372 | 0.7232× | 0.72× | ✅ |
| upload | 416,227 | 1,322,429 | 0.3147× | 0.32× | ✅ |
| static | 27,043 | 41,006 | 0.6595× | 0.65× | ✅ |
| client | 18,462 | 19,730 | 0.9358× | 0.93× | ✅ |
| socket | 19,694 | 21,045 | 0.9358× | 0.93× | ✅ |
| pgsql | 22,607 | 24,618 | 0.9183× | 0.92× | ✅ |
| sqlite | 23,401 | 25,236 | 0.9273× | 0.93× | ✅ |
| curl | 85,348 | 90,360 | 0.9445× | 0.95× | ✅ |
| dns | 230,071 | 232,808 | 0.9882× | 0.99× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
