# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,213 | 0.9017× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,165 | 0.9225× | 0.92× | ✅ |
| json | 22,050 | 23,173 | 0.9515× | 0.95× | ✅ |
| service-health | 16,308 | 17,786 | 0.9169× | 0.92× | ✅ |
| websocket | 6,736 | 8,192 | 0.8223× | 0.81× | ✅ |
| timer | 202,135 | 313,744 | 0.6443× | 0.65× | ✅ |
| app | 127,544 | 136,012 | 0.9377× | 0.93× | ✅ |
| http2 | 19,173 | 20,496 | 0.9354× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,023 | 155,723 | 0.8799× | 0.88× | ✅ |
| task | 32,076 | 33,268 | 0.9642× | 0.96× | ✅ |
| mysql | 50,294 | 51,084 | 0.9845× | 0.97× | ✅ |
| http-client | 33,654 | 35,794 | 0.9402× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,570 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,813 | 0.9768× | 0.98× | ✅ |
| wss | 101,564 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,272 | 0.9834× | 0.97× | ✅ |
| process-mode | 15,403 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 366,731 | 408,861 | 0.8970× | 0.89× | ✅ |
| websocket-64k | 90,040 | 159,692 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,893 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,393 | 41,028 | 0.6189× | 0.62× | ✅ |
| client | 18,592 | 19,721 | 0.9428× | 0.93× | ✅ |
| socket | 19,835 | 21,037 | 0.9429× | 0.93× | ✅ |
| pgsql | 22,889 | 24,663 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,075 | 90,422 | 0.9519× | 0.95× | ✅ |
| dns | 229,994 | 232,784 | 0.9880× | 0.99× | ✅ |
| file | 108,880 | 131,297 | 0.8293× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
