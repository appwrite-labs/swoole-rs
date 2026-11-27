# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5550ddd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,214 | 0.9017× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,165 | 0.9225× | 0.92× | ✅ |
| json | 22,047 | 23,173 | 0.9514× | 0.95× | ✅ |
| service-health | 16,305 | 17,786 | 0.9167× | 0.92× | ✅ |
| websocket | 6,736 | 8,192 | 0.8223× | 0.81× | ✅ |
| timer | 202,175 | 313,860 | 0.6442× | 0.65× | ✅ |
| app | 127,539 | 135,981 | 0.9379× | 0.93× | ✅ |
| http2 | 19,170 | 20,497 | 0.9353× | 0.92× | ✅ |
| table | 194,277 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,019 | 155,726 | 0.8799× | 0.88× | ✅ |
| task | 32,072 | 33,272 | 0.9639× | 0.96× | ✅ |
| mysql | 50,293 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,651 | 35,796 | 0.9401× | 0.93× | ✅ |
| gzip | 194,294 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,570 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,228 | 0.4954× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,491 | 18,734 | 0.9337× | 0.93× | ✅ |
| https | 106,288 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,570 | 104,804 | 0.9691× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,404 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 367,097 | 409,058 | 0.8974× | 0.89× | ✅ |
| websocket-64k | 90,039 | 159,695 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,966 | 16,368 | 0.7311× | 0.72× | ✅ |
| upload | 418,896 | 1,322,433 | 0.3168× | 0.32× | ✅ |
| static | 25,428 | 41,063 | 0.6192× | 0.62× | ✅ |
| client | 18,596 | 19,730 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,890 | 24,663 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,474 | 25,268 | 0.9290× | 0.93× | ✅ |
| curl | 86,083 | 90,418 | 0.9521× | 0.95× | ✅ |
| dns | 229,909 | 232,845 | 0.9874× | 0.99× | ✅ |
| file | 108,880 | 131,297 | 0.8293× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
