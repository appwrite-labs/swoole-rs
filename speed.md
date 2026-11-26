# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a705306` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,737 | 15,213 | 0.9029× | 0.91× | ✅ |
| echo-1k | 16,768 | 18,165 | 0.9231× | 0.92× | ✅ |
| json | 22,056 | 23,173 | 0.9518× | 0.95× | ✅ |
| service-health | 16,314 | 17,786 | 0.9172× | 0.92× | ✅ |
| websocket | 6,723 | 8,186 | 0.8214× | 0.81× | ✅ |
| timer | 202,127 | 313,711 | 0.6443× | 0.65× | ✅ |
| app | 127,526 | 135,980 | 0.9378× | 0.93× | ✅ |
| http2 | 19,163 | 20,501 | 0.9348× | 0.92× | ✅ |
| table | 194,290 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,342 | 155,726 | 0.8819× | 0.88× | ✅ |
| task | 32,099 | 33,265 | 0.9649× | 0.96× | ✅ |
| mysql | 50,300 | 51,088 | 0.9846× | 0.97× | ✅ |
| http-client | 33,664 | 35,793 | 0.9405× | 0.93× | ✅ |
| gzip | 194,259 | 197,095 | 0.9856× | 0.99× | ✅ |
| websocket-deflate | 179,389 | 181,570 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,507 | 18,730 | 0.9347× | 0.93× | ✅ |
| https | 106,253 | 108,813 | 0.9765× | 0.98× | ✅ |
| wss | 101,556 | 104,797 | 0.9691× | 0.97× | ✅ |
| atomic | 50,176 | 51,285 | 0.9784× | 0.97× | ✅ |
| process-mode | 15,412 | 17,024 | 0.9054× | 0.89× | ✅ |
| http2-concurrent | 366,985 | 409,057 | 0.8971× | 0.89× | ✅ |
| websocket-64k | 90,004 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7288× | 0.72× | ✅ |
| upload | 416,421 | 1,322,425 | 0.3149× | 0.32× | ✅ |
| static | 25,382 | 41,063 | 0.6181× | 0.64× | ✅ |
| client | 18,602 | 19,729 | 0.9428× | 0.93× | ✅ |
| socket | 19,844 | 21,045 | 0.9429× | 0.93× | ✅ |
| pgsql | 22,789 | 24,663 | 0.9240× | 0.92× | ✅ |
| sqlite | 23,488 | 25,268 | 0.9295× | 0.93× | ✅ |
| curl | 86,110 | 90,546 | 0.9510× | 0.95× | ✅ |
| dns | 229,870 | 232,867 | 0.9871× | 0.99× | ✅ |
| file | 108,886 | 131,207 | 0.8299× | 0.83× | ✅ |

Geometric mean 0.8531×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
