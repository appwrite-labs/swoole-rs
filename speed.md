# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,213 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,050 | 23,161 | 0.9521× | 0.95× | ✅ |
| service-health | 16,308 | 17,773 | 0.9176× | 0.92× | ✅ |
| websocket | 6,736 | 8,189 | 0.8226× | 0.81× | ✅ |
| timer | 201,998 | 313,732 | 0.6439× | 0.65× | ✅ |
| app | 127,532 | 136,003 | 0.9377× | 0.93× | ✅ |
| http2 | 19,170 | 20,496 | 0.9353× | 0.92× | ✅ |
| table | 194,345 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,019 | 155,717 | 0.8799× | 0.88× | ✅ |
| task | 32,076 | 33,267 | 0.9642× | 0.96× | ✅ |
| mysql | 50,290 | 51,085 | 0.9844× | 0.97× | ✅ |
| http-client | 33,656 | 35,800 | 0.9401× | 0.93× | ✅ |
| gzip | 194,292 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,988 | 181,573 | 0.9913× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9536× | 0.95× | ✅ |
| connect | 17,491 | 18,734 | 0.9336× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,574 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,420 | 51,276 | 0.9833× | 0.97× | ✅ |
| process-mode | 15,404 | 17,025 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 366,624 | 408,864 | 0.8967× | 0.89× | ✅ |
| websocket-64k | 90,037 | 159,695 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,893 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,364 | 40,999 | 0.6186× | 0.62× | ✅ |
| client | 18,593 | 19,720 | 0.9428× | 0.93× | ✅ |
| socket | 19,835 | 21,036 | 0.9429× | 0.93× | ✅ |
| pgsql | 22,888 | 24,663 | 0.9280× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,074 | 90,420 | 0.9519× | 0.95× | ✅ |
| dns | 230,106 | 232,954 | 0.9878× | 0.99× | ✅ |
| file | 108,876 | 130,922 | 0.8316× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
