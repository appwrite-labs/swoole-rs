# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0817578` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,861 | 15,213 | 0.9111× | 0.92× | ✅ |
| echo-1k | 16,900 | 18,165 | 0.9304× | 0.94× | ✅ |
| json | 22,020 | 23,161 | 0.9507× | 0.95× | ✅ |
| service-health | 16,458 | 17,777 | 0.9258× | 0.93× | ✅ |
| websocket | 6,553 | 8,188 | 0.8003× | 0.8× | ✅ |
| timer | 205,807 | 313,572 | 0.6563× | 0.65× | ✅ |
| app | 125,626 | 135,970 | 0.9239× | 0.93× | ✅ |
| http2 | 18,937 | 20,464 | 0.9254× | 0.91× | ✅ |
| table | 194,672 | 201,865 | 0.9644× | 0.97× | ✅ |
| channel | 136,821 | 155,174 | 0.8817× | 0.88× | ✅ |
| task | 32,399 | 33,264 | 0.9740× | 0.96× | ✅ |
| mysql | 49,566 | 51,058 | 0.9708× | 0.97× | ✅ |
| http-client | 33,563 | 35,804 | 0.9374× | 0.93× | ✅ |
| gzip | 194,028 | 197,079 | 0.9845× | 0.99× | ✅ |
| websocket-deflate | 178,819 | 181,568 | 0.9849× | 0.99× | ✅ |
| tcp-length | 4,652 | 4,762 | 0.9769× | 0.98× | ✅ |
| tcp-eof | 5,142 | 10,231 | 0.5026× | 0.51× | ✅ |
| udp | 7,515 | 7,987 | 0.9410× | 0.93× | ✅ |
| connect | 17,638 | 18,707 | 0.9429× | 0.94× | ✅ |
| https | 108,129 | 108,803 | 0.9938× | 1.0× | ✅ |
| wss | 103,741 | 104,797 | 0.9899× | 0.99× | ✅ |
| atomic | 49,968 | 51,316 | 0.9737× | 0.97× | ✅ |
| process-mode | 15,053 | 16,960 | 0.8876× | 0.88× | ✅ |
| http2-concurrent | 358,270 | 409,694 | 0.8745× | 0.88× | ✅ |
| websocket-64k | 89,847 | 159,697 | 0.5626× | 0.57× | ✅ |
| websocket-fragmented | 11,975 | 16,327 | 0.7334× | 0.73× | ✅ |
| upload | 414,279 | 1,322,388 | 0.3133× | 0.32× | ✅ |
| static | 26,973 | 41,080 | 0.6566× | 0.66× | ✅ |
| client | 18,539 | 19,754 | 0.9385× | 0.94× | ✅ |
| socket | 19,715 | 21,069 | 0.9357× | 0.92× | ✅ |
| pgsql | 23,302 | 24,595 | 0.9474× | 0.95× | ✅ |
| sqlite | 23,533 | 25,236 | 0.9325× | 0.93× | ✅ |
| curl | 86,017 | 90,482 | 0.9507× | 0.94× | ✅ |
| dns | 230,771 | 232,589 | 0.9922× | 0.99× | ✅ |

Geometric mean 0.8553×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
