# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7c8788c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,785 | 15,210 | 0.9063× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,162 | 0.9252× | 0.93× | ✅ |
| json | 21,945 | 23,161 | 0.9475× | 0.95× | ✅ |
| service-health | 16,358 | 17,773 | 0.9204× | 0.92× | ✅ |
| websocket | 6,539 | 8,191 | 0.7983× | 0.8× | ✅ |
| timer | 206,165 | 313,532 | 0.6576× | 0.65× | ✅ |
| app | 126,537 | 135,864 | 0.9313× | 0.93× | ✅ |
| http2 | 19,106 | 20,464 | 0.9337× | 0.92× | ✅ |
| table | 194,362 | 201,865 | 0.9628× | 0.97× | ✅ |
| channel | 137,572 | 155,174 | 0.8866× | 0.88× | ✅ |
| task | 32,485 | 33,263 | 0.9766× | 0.96× | ✅ |
| mysql | 49,833 | 51,054 | 0.9761× | 0.97× | ✅ |
| http-client | 33,515 | 35,800 | 0.9362× | 0.93× | ✅ |
| gzip | 194,442 | 197,079 | 0.9866× | 0.99× | ✅ |
| websocket-deflate | 178,991 | 181,560 | 0.9858× | 0.99× | ✅ |
| tcp-length | 4,574 | 4,762 | 0.9605× | 0.96× | ✅ |
| tcp-eof | 5,154 | 10,231 | 0.5037× | 0.5× | ✅ |
| udp | 7,473 | 7,986 | 0.9358× | 0.93× | ✅ |
| connect | 17,493 | 18,707 | 0.9351× | 0.93× | ✅ |
| https | 105,946 | 108,803 | 0.9737× | 0.98× | ✅ |
| wss | 100,979 | 104,794 | 0.9636× | 0.97× | ✅ |
| atomic | 49,857 | 51,312 | 0.9716× | 0.97× | ✅ |
| process-mode | 15,015 | 17,022 | 0.8821× | 0.88× | ✅ |
| http2-concurrent | 365,689 | 408,880 | 0.8944× | 0.88× | ✅ |
| websocket-64k | 89,791 | 159,698 | 0.5623× | 0.57× | ✅ |
| websocket-fragmented | 11,609 | 16,328 | 0.7110× | 0.72× | ✅ |
| upload | 415,250 | 1,322,389 | 0.3140× | 0.32× | ✅ |
| static | 26,686 | 41,045 | 0.6502× | 0.65× | ✅ |
| client | 18,510 | 19,757 | 0.9369× | 0.94× | ✅ |
| socket | 19,668 | 21,072 | 0.9334× | 0.92× | ✅ |
| pgsql | 22,791 | 24,598 | 0.9265× | 0.93× | ✅ |
| sqlite | 23,445 | 25,236 | 0.9290× | 0.93× | ✅ |
| curl | 86,346 | 90,472 | 0.9544× | 0.94× | ✅ |
| dns | 230,005 | 232,778 | 0.9881× | 0.99× | ✅ |

Geometric mean 0.8523×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
