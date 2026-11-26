# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5550ddd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,214 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,754 | 18,165 | 0.9223× | 0.92× | ✅ |
| json | 22,047 | 23,173 | 0.9514× | 0.95× | ✅ |
| service-health | 16,305 | 17,786 | 0.9167× | 0.92× | ✅ |
| websocket | 6,736 | 8,189 | 0.8226× | 0.81× | ✅ |
| timer | 202,142 | 314,018 | 0.6437× | 0.65× | ✅ |
| app | 127,585 | 135,938 | 0.9386× | 0.93× | ✅ |
| http2 | 19,171 | 20,501 | 0.9351× | 0.92× | ✅ |
| table | 194,277 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,023 | 155,726 | 0.8799× | 0.88× | ✅ |
| task | 32,073 | 33,268 | 0.9641× | 0.96× | ✅ |
| mysql | 50,294 | 51,084 | 0.9845× | 0.97× | ✅ |
| http-client | 33,653 | 35,789 | 0.9403× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,570 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,574 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,404 | 17,021 | 0.9050× | 0.89× | ✅ |
| http2-concurrent | 367,067 | 409,057 | 0.8974× | 0.89× | ✅ |
| websocket-64k | 90,035 | 159,694 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,966 | 16,369 | 0.7311× | 0.72× | ✅ |
| upload | 418,896 | 1,322,428 | 0.3168× | 0.32× | ✅ |
| static | 25,393 | 41,028 | 0.6189× | 0.62× | ✅ |
| client | 18,596 | 19,726 | 0.9427× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,890 | 24,663 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,478 | 25,264 | 0.9293× | 0.93× | ✅ |
| curl | 86,077 | 90,419 | 0.9520× | 0.95× | ✅ |
| dns | 230,212 | 232,906 | 0.9884× | 0.99× | ✅ |
| file | 108,880 | 131,208 | 0.8298× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
