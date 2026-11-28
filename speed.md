# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,714 | 15,213 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,165 | 0.9225× | 0.92× | ✅ |
| json | 22,051 | 23,161 | 0.9521× | 0.95× | ✅ |
| service-health | 16,308 | 17,773 | 0.9176× | 0.92× | ✅ |
| websocket | 6,740 | 8,192 | 0.8227× | 0.81× | ✅ |
| timer | 202,055 | 313,824 | 0.6438× | 0.65× | ✅ |
| app | 127,562 | 135,900 | 0.9386× | 0.93× | ✅ |
| http2 | 19,167 | 20,500 | 0.9350× | 0.92× | ✅ |
| table | 194,345 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,019 | 155,717 | 0.8799× | 0.88× | ✅ |
| task | 32,077 | 33,272 | 0.9641× | 0.96× | ✅ |
| mysql | 50,290 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,653 | 35,797 | 0.9401× | 0.93× | ✅ |
| gzip | 194,290 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,573 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,228 | 0.4954× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,491 | 18,734 | 0.9336× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,574 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,420 | 51,285 | 0.9831× | 0.97× | ✅ |
| process-mode | 15,407 | 17,025 | 0.9049× | 0.89× | ✅ |
| http2-concurrent | 366,894 | 409,057 | 0.8969× | 0.89× | ✅ |
| websocket-64k | 90,036 | 159,694 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,368 | 0.7309× | 0.72× | ✅ |
| upload | 418,910 | 1,322,425 | 0.3168× | 0.32× | ✅ |
| static | 25,416 | 41,063 | 0.6189× | 0.62× | ✅ |
| client | 18,596 | 19,729 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,046 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,887 | 24,662 | 0.9280× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,084 | 90,418 | 0.9521× | 0.95× | ✅ |
| dns | 229,756 | 232,715 | 0.9873× | 0.99× | ✅ |
| file | 108,880 | 131,296 | 0.8293× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
