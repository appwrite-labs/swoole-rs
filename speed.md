# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5550ddd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,214 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,754 | 18,165 | 0.9223× | 0.92× | ✅ |
| json | 22,047 | 23,173 | 0.9514× | 0.95× | ✅ |
| service-health | 16,305 | 17,786 | 0.9167× | 0.92× | ✅ |
| websocket | 6,736 | 8,185 | 0.8230× | 0.81× | ✅ |
| timer | 202,184 | 313,808 | 0.6443× | 0.65× | ✅ |
| app | 127,566 | 135,967 | 0.9382× | 0.93× | ✅ |
| http2 | 19,178 | 20,494 | 0.9358× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,019 | 155,726 | 0.8799× | 0.88× | ✅ |
| task | 32,073 | 33,272 | 0.9640× | 0.96× | ✅ |
| mysql | 50,290 | 51,086 | 0.9844× | 0.97× | ✅ |
| http-client | 33,657 | 35,797 | 0.9402× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,570 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9536× | 0.95× | ✅ |
| connect | 17,491 | 18,734 | 0.9336× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,574 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,281 | 0.9833× | 0.97× | ✅ |
| process-mode | 15,403 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 366,942 | 409,474 | 0.8961× | 0.89× | ✅ |
| websocket-64k | 90,036 | 159,691 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,906 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,381 | 41,028 | 0.6186× | 0.62× | ✅ |
| client | 18,596 | 19,730 | 0.9425× | 0.93× | ✅ |
| socket | 19,836 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,887 | 24,663 | 0.9280× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,074 | 90,422 | 0.9519× | 0.95× | ✅ |
| dns | 229,454 | 232,526 | 0.9868× | 0.99× | ✅ |
| file | 108,877 | 131,211 | 0.8298× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
