# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5550ddd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,214 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,050 | 23,161 | 0.9521× | 0.95× | ✅ |
| service-health | 16,308 | 17,773 | 0.9176× | 0.92× | ✅ |
| websocket | 6,740 | 8,189 | 0.8231× | 0.81× | ✅ |
| timer | 202,169 | 313,899 | 0.6441× | 0.65× | ✅ |
| app | 127,570 | 135,920 | 0.9386× | 0.93× | ✅ |
| http2 | 19,177 | 20,497 | 0.9356× | 0.92× | ✅ |
| table | 194,345 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,019 | 155,717 | 0.8799× | 0.88× | ✅ |
| task | 32,072 | 33,272 | 0.9639× | 0.96× | ✅ |
| mysql | 50,265 | 51,088 | 0.9839× | 0.97× | ✅ |
| http-client | 33,657 | 35,793 | 0.9403× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,570 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,574 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,420 | 51,285 | 0.9831× | 0.97× | ✅ |
| process-mode | 15,404 | 17,028 | 0.9046× | 0.89× | ✅ |
| http2-concurrent | 367,019 | 409,057 | 0.8972× | 0.89× | ✅ |
| websocket-64k | 90,036 | 159,693 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,907 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,445 | 41,092 | 0.6192× | 0.62× | ✅ |
| client | 18,596 | 19,729 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,888 | 24,663 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,474 | 25,268 | 0.9290× | 0.93× | ✅ |
| curl | 86,084 | 90,547 | 0.9507× | 0.95× | ✅ |
| dns | 230,071 | 232,771 | 0.9884× | 0.99× | ✅ |
| file | 108,880 | 130,917 | 0.8317× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
