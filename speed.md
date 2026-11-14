# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `1e0c1a7` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,967 | 15,214 | 0.9181× | 0.92× | ✅ |
| echo-1k | 17,051 | 18,166 | 0.9386× | 0.94× | ✅ |
| json | 22,103 | 23,161 | 0.9543× | 0.95× | ✅ |
| service-health | 16,537 | 17,777 | 0.9303× | 0.93× | ✅ |
| websocket | 6,633 | 8,188 | 0.8101× | 0.8× | ✅ |
| timer | 205,915 | 313,497 | 0.6568× | 0.65× | ✅ |
| app | 125,555 | 135,912 | 0.9238× | 0.93× | ✅ |
| http2 | 18,683 | 20,468 | 0.9128× | 0.91× | ✅ |
| table | 196,000 | 201,865 | 0.9709× | 0.97× | ✅ |
| channel | 136,691 | 155,170 | 0.8809× | 0.89× | ✅ |
| task | 32,498 | 33,263 | 0.9770× | 0.96× | ✅ |
| mysql | 49,688 | 51,061 | 0.9731× | 0.97× | ✅ |
| http-client | 33,893 | 35,801 | 0.9467× | 0.94× | ✅ |
| client | 18,590 | 19,757 | 0.9409× | 0.94× | ✅ |
| socket | 19,710 | 21,072 | 0.9353× | 0.92× | ✅ |
| gzip | 194,166 | 197,083 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,094 | 181,572 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,765 | 0.9899× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,986 | 0.9445× | 0.93× | ✅ |
| connect | 17,744 | 18,703 | 0.9487× | 0.94× | ✅ |
| https | 108,216 | 108,810 | 0.9945× | 1.0× | ✅ |
| wss | 103,855 | 104,800 | 0.9910× | 0.99× | ✅ |
| atomic | 49,694 | 51,316 | 0.9684× | 0.97× | ✅ |
| process-mode | 15,090 | 17,025 | 0.8863× | 0.88× | ✅ |
| http2-concurrent | 361,242 | 409,069 | 0.8831× | 0.88× | ✅ |
| websocket-64k | 89,958 | 159,696 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,870 | 16,328 | 0.7270× | 0.74× | ✅ |
| upload | 413,440 | 1,322,389 | 0.3126× | 0.32× | ✅ |
| static | 27,313 | 40,987 | 0.6664× | 0.66× | ✅ |
| pgsql | 23,382 | 24,595 | 0.9507× | 0.95× | ✅ |
| sqlite | 23,555 | 25,236 | 0.9334× | 0.93× | ✅ |
| curl | 86,142 | 90,350 | 0.9534× | 0.94× | ✅ |
| dns | 231,439 | 233,008 | 0.9933× | 0.99× | ✅ |

Geometric mean 0.8580×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
