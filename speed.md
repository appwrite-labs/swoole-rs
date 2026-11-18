# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f479c7f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,743 | 15,214 | 0.9033× | 0.91× | ✅ |
| echo-1k | 16,743 | 18,162 | 0.9218× | 0.92× | ✅ |
| json | 22,072 | 23,170 | 0.9526× | 0.95× | ✅ |
| service-health | 16,318 | 17,786 | 0.9175× | 0.92× | ✅ |
| websocket | 6,681 | 8,189 | 0.8158× | 0.81× | ✅ |
| timer | 201,397 | 313,729 | 0.6419× | 0.65× | ✅ |
| app | 127,005 | 135,897 | 0.9346× | 0.93× | ✅ |
| http2 | 18,972 | 20,497 | 0.9256× | 0.92× | ✅ |
| table | 193,903 | 201,885 | 0.9605× | 0.96× | ✅ |
| channel | 137,969 | 155,723 | 0.8860× | 0.88× | ✅ |
| task | 31,950 | 33,269 | 0.9604× | 0.96× | ✅ |
| mysql | 50,065 | 51,085 | 0.9800× | 0.97× | ✅ |
| http-client | 33,406 | 35,796 | 0.9332× | 0.93× | ✅ |
| gzip | 194,361 | 197,095 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,766 | 0.9744× | 0.96× | ✅ |
| tcp-eof | 5,201 | 10,231 | 0.5083× | 0.5× | ✅ |
| udp | 7,579 | 7,987 | 0.9490× | 0.95× | ✅ |
| connect | 17,581 | 18,730 | 0.9387× | 0.93× | ✅ |
| https | 106,026 | 108,809 | 0.9744× | 0.98× | ✅ |
| wss | 101,243 | 104,800 | 0.9661× | 0.97× | ✅ |
| atomic | 50,173 | 51,276 | 0.9785× | 0.97× | ✅ |
| process-mode | 15,264 | 17,024 | 0.8966× | 0.89× | ✅ |
| http2-concurrent | 364,099 | 408,866 | 0.8905× | 0.89× | ✅ |
| websocket-64k | 89,980 | 159,691 | 0.5635× | 0.57× | ✅ |
| websocket-fragmented | 11,840 | 16,372 | 0.7232× | 0.72× | ✅ |
| upload | 416,434 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,070 | 41,028 | 0.6598× | 0.65× | ✅ |
| client | 18,472 | 19,720 | 0.9367× | 0.93× | ✅ |
| socket | 19,700 | 21,037 | 0.9365× | 0.93× | ✅ |
| pgsql | 22,919 | 24,618 | 0.9310× | 0.92× | ✅ |
| sqlite | 23,407 | 25,236 | 0.9275× | 0.93× | ✅ |
| curl | 85,354 | 90,174 | 0.9465× | 0.95× | ✅ |
| dns | 229,978 | 232,786 | 0.9879× | 0.99× | ✅ |

Geometric mean 0.8540×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
