# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5550ddd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,210 | 0.9019× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,165 | 0.9225× | 0.92× | ✅ |
| json | 22,047 | 23,165 | 0.9517× | 0.95× | ✅ |
| service-health | 16,305 | 17,777 | 0.9172× | 0.92× | ✅ |
| websocket | 6,736 | 8,189 | 0.8226× | 0.81× | ✅ |
| timer | 202,194 | 313,877 | 0.6442× | 0.65× | ✅ |
| app | 127,538 | 135,877 | 0.9386× | 0.93× | ✅ |
| http2 | 19,168 | 20,500 | 0.9350× | 0.92× | ✅ |
| table | 194,345 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,019 | 155,717 | 0.8799× | 0.88× | ✅ |
| task | 32,073 | 33,269 | 0.9641× | 0.96× | ✅ |
| mysql | 50,294 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,652 | 35,790 | 0.9403× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,025 | 181,573 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9535× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,813 | 0.9768× | 0.98× | ✅ |
| wss | 101,570 | 104,800 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,403 | 17,021 | 0.9049× | 0.89× | ✅ |
| http2-concurrent | 367,021 | 409,662 | 0.8959× | 0.89× | ✅ |
| websocket-64k | 90,046 | 159,695 | 0.5639× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,910 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,352 | 40,999 | 0.6183× | 0.62× | ✅ |
| client | 18,592 | 19,730 | 0.9423× | 0.93× | ✅ |
| socket | 19,835 | 21,046 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,889 | 24,662 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,478 | 25,253 | 0.9297× | 0.93× | ✅ |
| curl | 86,073 | 90,418 | 0.9519× | 0.95× | ✅ |
| dns | 230,201 | 232,550 | 0.9899× | 0.99× | ✅ |
| file | 108,880 | 131,300 | 0.8292× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
