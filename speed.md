# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5550ddd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,714 | 15,214 | 0.9014× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,051 | 23,170 | 0.9517× | 0.95× | ✅ |
| service-health | 16,308 | 17,782 | 0.9171× | 0.92× | ✅ |
| websocket | 6,740 | 8,192 | 0.8227× | 0.81× | ✅ |
| timer | 202,206 | 313,999 | 0.6440× | 0.65× | ✅ |
| app | 127,588 | 135,916 | 0.9387× | 0.93× | ✅ |
| http2 | 19,173 | 20,496 | 0.9354× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,019 | 155,723 | 0.8799× | 0.88× | ✅ |
| task | 32,076 | 33,265 | 0.9643× | 0.96× | ✅ |
| mysql | 50,294 | 51,085 | 0.9845× | 0.97× | ✅ |
| http-client | 33,653 | 35,796 | 0.9401× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,025 | 181,573 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9536× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,802 | 0.9769× | 0.98× | ✅ |
| wss | 101,570 | 104,800 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,276 | 0.9834× | 0.97× | ✅ |
| process-mode | 15,403 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 366,846 | 408,865 | 0.8972× | 0.89× | ✅ |
| websocket-64k | 90,034 | 159,695 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,966 | 16,368 | 0.7311× | 0.72× | ✅ |
| upload | 418,898 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,364 | 40,999 | 0.6186× | 0.62× | ✅ |
| client | 18,596 | 19,721 | 0.9430× | 0.93× | ✅ |
| socket | 19,832 | 21,036 | 0.9427× | 0.93× | ✅ |
| pgsql | 22,891 | 24,663 | 0.9282× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,078 | 90,416 | 0.9520× | 0.95× | ✅ |
| dns | 229,938 | 232,563 | 0.9887× | 0.99× | ✅ |
| file | 108,876 | 131,300 | 0.8292× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
