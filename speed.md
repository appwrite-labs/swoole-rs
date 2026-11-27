# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4d4694d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,214 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,050 | 23,161 | 0.9521× | 0.95× | ✅ |
| service-health | 16,308 | 17,777 | 0.9174× | 0.92× | ✅ |
| websocket | 6,740 | 8,189 | 0.8231× | 0.81× | ✅ |
| timer | 202,139 | 313,757 | 0.6443× | 0.65× | ✅ |
| app | 127,534 | 135,983 | 0.9379× | 0.93× | ✅ |
| http2 | 19,178 | 20,496 | 0.9357× | 0.92× | ✅ |
| table | 194,345 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,019 | 155,714 | 0.8799× | 0.88× | ✅ |
| task | 32,076 | 33,265 | 0.9643× | 0.96× | ✅ |
| mysql | 50,287 | 51,085 | 0.9844× | 0.97× | ✅ |
| http-client | 33,656 | 35,793 | 0.9403× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,570 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,764 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,561 | 104,806 | 0.9690× | 0.97× | ✅ |
| atomic | 50,420 | 51,285 | 0.9831× | 0.97× | ✅ |
| process-mode | 15,403 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 366,976 | 409,060 | 0.8971× | 0.89× | ✅ |
| websocket-64k | 90,038 | 159,691 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,892 | 1,322,428 | 0.3168× | 0.32× | ✅ |
| static | 25,364 | 40,999 | 0.6186× | 0.62× | ✅ |
| client | 18,596 | 19,730 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,885 | 24,663 | 0.9279× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,084 | 90,422 | 0.9520× | 0.95× | ✅ |
| dns | 230,232 | 232,434 | 0.9905× | 0.99× | ✅ |
| file | 108,880 | 131,300 | 0.8292× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
