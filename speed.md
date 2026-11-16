# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2edc46a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,918 | 15,214 | 0.9149× | 0.91× | ✅ |
| echo-1k | 16,948 | 18,165 | 0.9330× | 0.93× | ✅ |
| json | 22,055 | 23,158 | 0.9524× | 0.95× | ✅ |
| service-health | 16,502 | 17,773 | 0.9285× | 0.93× | ✅ |
| websocket | 6,565 | 8,191 | 0.8015× | 0.8× | ✅ |
| timer | 206,553 | 313,583 | 0.6587× | 0.65× | ✅ |
| app | 126,338 | 135,895 | 0.9297× | 0.93× | ✅ |
| http2 | 19,005 | 20,467 | 0.9286× | 0.92× | ✅ |
| table | 195,161 | 201,865 | 0.9668× | 0.97× | ✅ |
| channel | 137,539 | 155,170 | 0.8864× | 0.88× | ✅ |
| task | 32,480 | 33,264 | 0.9764× | 0.96× | ✅ |
| mysql | 49,643 | 51,060 | 0.9722× | 0.97× | ✅ |
| http-client | 33,584 | 35,803 | 0.9380× | 0.93× | ✅ |
| gzip | 194,030 | 197,079 | 0.9845× | 0.99× | ✅ |
| websocket-deflate | 178,964 | 181,572 | 0.9856× | 0.99× | ✅ |
| tcp-length | 4,640 | 4,765 | 0.9737× | 0.98× | ✅ |
| tcp-eof | 5,203 | 10,231 | 0.5086× | 0.51× | ✅ |
| udp | 7,539 | 7,987 | 0.9440× | 0.93× | ✅ |
| connect | 17,858 | 18,707 | 0.9546× | 0.94× | ✅ |
| https | 108,116 | 108,810 | 0.9936× | 1.0× | ✅ |
| wss | 103,670 | 104,794 | 0.9893× | 0.99× | ✅ |
| atomic | 50,018 | 51,312 | 0.9748× | 0.97× | ✅ |
| process-mode | 15,139 | 16,963 | 0.8925× | 0.88× | ✅ |
| http2-concurrent | 361,642 | 409,069 | 0.8841× | 0.88× | ✅ |
| websocket-64k | 89,866 | 159,694 | 0.5627× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,331 | 0.7326× | 0.72× | ✅ |
| upload | 414,012 | 1,322,392 | 0.3131× | 0.32× | ✅ |
| static | 27,050 | 40,993 | 0.6599× | 0.66× | ✅ |
| client | 18,597 | 19,757 | 0.9413× | 0.94× | ✅ |
| socket | 19,754 | 21,072 | 0.9375× | 0.92× | ✅ |
| pgsql | 23,480 | 24,594 | 0.9547× | 0.95× | ✅ |
| sqlite | 23,597 | 25,236 | 0.9350× | 0.93× | ✅ |
| curl | 85,861 | 90,351 | 0.9503× | 0.94× | ✅ |
| dns | 230,518 | 232,453 | 0.9917× | 0.99× | ✅ |

Geometric mean 0.8578×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
