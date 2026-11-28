# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,214 | 0.9017× | 0.91× | ✅ |
| echo-1k | 16,754 | 18,165 | 0.9223× | 0.92× | ✅ |
| json | 22,047 | 23,174 | 0.9514× | 0.95× | ✅ |
| service-health | 16,308 | 17,786 | 0.9169× | 0.92× | ✅ |
| websocket | 6,740 | 8,189 | 0.8231× | 0.81× | ✅ |
| timer | 202,181 | 313,948 | 0.6440× | 0.65× | ✅ |
| app | 127,573 | 135,997 | 0.9381× | 0.93× | ✅ |
| http2 | 19,177 | 20,496 | 0.9356× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,019 | 155,726 | 0.8799× | 0.88× | ✅ |
| task | 32,072 | 33,268 | 0.9641× | 0.96× | ✅ |
| mysql | 50,291 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,653 | 35,790 | 0.9403× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,025 | 181,573 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9536× | 0.95× | ✅ |
| connect | 17,494 | 18,730 | 0.9340× | 0.93× | ✅ |
| https | 106,288 | 108,813 | 0.9768× | 0.98× | ✅ |
| wss | 101,574 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,404 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 366,991 | 409,058 | 0.8972× | 0.89× | ✅ |
| websocket-64k | 90,043 | 159,691 | 0.5639× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,892 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,457 | 41,092 | 0.6195× | 0.62× | ✅ |
| client | 18,593 | 19,730 | 0.9424× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,889 | 24,663 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,474 | 25,268 | 0.9290× | 0.93× | ✅ |
| curl | 86,078 | 90,418 | 0.9520× | 0.95× | ✅ |
| dns | 229,852 | 232,550 | 0.9884× | 0.99× | ✅ |
| file | 108,880 | 131,296 | 0.8293× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
