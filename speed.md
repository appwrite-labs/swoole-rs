# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,210 | 0.9019× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,165 | 0.9225× | 0.92× | ✅ |
| json | 22,047 | 23,173 | 0.9514× | 0.95× | ✅ |
| service-health | 16,305 | 17,786 | 0.9167× | 0.92× | ✅ |
| websocket | 6,736 | 8,189 | 0.8226× | 0.81× | ✅ |
| timer | 202,129 | 313,880 | 0.6440× | 0.65× | ✅ |
| app | 127,578 | 135,966 | 0.9383× | 0.93× | ✅ |
| http2 | 19,181 | 20,500 | 0.9357× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,019 | 155,726 | 0.8799× | 0.88× | ✅ |
| task | 32,073 | 33,272 | 0.9640× | 0.96× | ✅ |
| mysql | 50,294 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,653 | 35,794 | 0.9402× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,570 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9536× | 0.95× | ✅ |
| connect | 17,495 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,813 | 0.9768× | 0.98× | ✅ |
| wss | 101,566 | 104,804 | 0.9691× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,404 | 17,021 | 0.9050× | 0.89× | ✅ |
| http2-concurrent | 367,002 | 409,057 | 0.8972× | 0.89× | ✅ |
| websocket-64k | 90,037 | 159,694 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,893 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,457 | 41,063 | 0.6199× | 0.62× | ✅ |
| client | 18,598 | 19,729 | 0.9427× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,890 | 24,662 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,072 | 90,417 | 0.9519× | 0.95× | ✅ |
| dns | 229,807 | 233,339 | 0.9849× | 0.99× | ✅ |
| file | 108,880 | 131,297 | 0.8293× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
