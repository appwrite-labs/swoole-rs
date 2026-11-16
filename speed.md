# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b3264e8` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,862 | 15,210 | 0.9114× | 0.91× | ✅ |
| echo-1k | 16,896 | 18,165 | 0.9301× | 0.93× | ✅ |
| json | 22,019 | 23,161 | 0.9507× | 0.95× | ✅ |
| service-health | 16,448 | 17,773 | 0.9254× | 0.93× | ✅ |
| websocket | 6,543 | 8,191 | 0.7988× | 0.8× | ✅ |
| timer | 206,065 | 313,600 | 0.6571× | 0.65× | ✅ |
| app | 125,569 | 135,891 | 0.9240× | 0.93× | ✅ |
| http2 | 18,955 | 20,465 | 0.9263× | 0.91× | ✅ |
| table | 195,109 | 201,865 | 0.9665× | 0.97× | ✅ |
| channel | 136,810 | 155,167 | 0.8817× | 0.88× | ✅ |
| task | 32,411 | 33,264 | 0.9744× | 0.96× | ✅ |
| mysql | 49,630 | 51,055 | 0.9721× | 0.97× | ✅ |
| http-client | 33,486 | 35,804 | 0.9353× | 0.93× | ✅ |
| gzip | 193,982 | 197,079 | 0.9843× | 0.99× | ✅ |
| websocket-deflate | 178,851 | 181,564 | 0.9851× | 0.99× | ✅ |
| tcp-length | 4,633 | 4,765 | 0.9722× | 0.98× | ✅ |
| tcp-eof | 5,194 | 10,231 | 0.5077× | 0.51× | ✅ |
| udp | 7,519 | 7,986 | 0.9415× | 0.93× | ✅ |
| connect | 17,644 | 18,707 | 0.9432× | 0.94× | ✅ |
| https | 108,126 | 108,803 | 0.9938× | 1.0× | ✅ |
| wss | 103,707 | 104,794 | 0.9896× | 0.99× | ✅ |
| atomic | 49,960 | 51,316 | 0.9736× | 0.97× | ✅ |
| process-mode | 15,060 | 16,957 | 0.8882× | 0.88× | ✅ |
| http2-concurrent | 360,237 | 409,073 | 0.8806× | 0.88× | ✅ |
| websocket-64k | 89,850 | 159,693 | 0.5626× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,327 | 0.7306× | 0.72× | ✅ |
| upload | 413,997 | 1,322,387 | 0.3131× | 0.32× | ✅ |
| static | 26,980 | 41,080 | 0.6568× | 0.66× | ✅ |
| client | 18,535 | 19,754 | 0.9383× | 0.94× | ✅ |
| socket | 19,693 | 21,069 | 0.9347× | 0.92× | ✅ |
| pgsql | 23,288 | 24,597 | 0.9468× | 0.95× | ✅ |
| sqlite | 23,526 | 25,221 | 0.9328× | 0.93× | ✅ |
| curl | 86,002 | 90,352 | 0.9519× | 0.94× | ✅ |
| dns | 230,759 | 232,811 | 0.9912× | 0.99× | ✅ |

Geometric mean 0.8556×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
