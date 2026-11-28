# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,210 | 0.9019× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,050 | 23,161 | 0.9521× | 0.95× | ✅ |
| service-health | 16,308 | 17,773 | 0.9176× | 0.92× | ✅ |
| websocket | 6,740 | 8,192 | 0.8227× | 0.81× | ✅ |
| timer | 202,111 | 313,837 | 0.6440× | 0.65× | ✅ |
| app | 127,573 | 135,982 | 0.9382× | 0.93× | ✅ |
| http2 | 19,173 | 20,496 | 0.9354× | 0.92× | ✅ |
| table | 194,345 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,023 | 155,717 | 0.8799× | 0.88× | ✅ |
| task | 32,071 | 33,272 | 0.9639× | 0.96× | ✅ |
| mysql | 50,294 | 51,074 | 0.9847× | 0.97× | ✅ |
| http-client | 33,653 | 35,793 | 0.9402× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,025 | 181,573 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,491 | 18,734 | 0.9336× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,572 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,407 | 17,024 | 0.9050× | 0.89× | ✅ |
| http2-concurrent | 367,111 | 409,053 | 0.8975× | 0.89× | ✅ |
| websocket-64k | 90,037 | 159,691 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,368 | 0.7309× | 0.72× | ✅ |
| upload | 418,896 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,457 | 41,092 | 0.6195× | 0.62× | ✅ |
| client | 18,597 | 19,730 | 0.9426× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,889 | 24,662 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,075 | 90,403 | 0.9521× | 0.95× | ✅ |
| dns | 229,938 | 233,259 | 0.9858× | 0.99× | ✅ |
| file | 108,880 | 131,208 | 0.8298× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
