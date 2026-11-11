# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `47f8fc3` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,256 | 15,213 | 0.9371× | 0.93× | ✅ |
| echo-1k | 17,374 | 18,166 | 0.9565× | 0.95× | ✅ |
| json | 22,351 | 23,161 | 0.9650× | 0.96× | ✅ |
| service-health | 16,849 | 17,777 | 0.9478× | 0.94× | ✅ |
| websocket | 6,680 | 8,185 | 0.8160× | 0.83× | ✅ |
| timer | 202,285 | 313,587 | 0.6451× | 0.65× | ✅ |
| app | 126,462 | 135,881 | 0.9307× | 0.93× | ✅ |
| http2 | 20,022 | 20,467 | 0.9783× | 0.98× | ✅ |
| table | 196,093 | 201,865 | 0.9714× | 0.97× | ✅ |
| channel | 139,821 | 154,654 | 0.9041× | 0.91× | ✅ |
| task | 32,147 | 33,263 | 0.9664× | 0.97× | ✅ |
| mysql | 49,890 | 51,059 | 0.9771× | 0.98× | ✅ |
| http-client | 34,100 | 35,809 | 0.9523× | 0.95× | ✅ |
| client | 18,857 | 19,758 | 0.9544× | 0.95× | ✅ |
| socket | 19,836 | 21,068 | 0.9415× | 0.94× | ✅ |
| gzip | 196,231 | 197,079 | 0.9957× | 1.0× | ✅ |
| websocket-deflate | 181,441 | 181,568 | 0.9993× | 1.0× | ✅ |
| tcp-length | 5,168 | 4,762 | 1.0851× | 1.1× | ➖ |
| tcp-eof | 5,699 | 10,231 | 0.5570× | 0.57× | ✅ |
| udp | 7,953 | 7,986 | 0.9958× | 1.0× | ✅ |
| connect | 18,417 | 18,707 | 0.9845× | 0.99× | ✅ |
| https | 109,184 | 108,806 | 1.0035× | 1.01× | ➖ |
| wss | 104,585 | 104,794 | 0.9980× | 1.0× | ✅ |
| atomic | 50,048 | 51,316 | 0.9753× | 0.98× | ✅ |
| process-mode | 16,657 | 16,957 | 0.9823× | 0.98× | ✅ |
| http2-concurrent | 376,133 | 408,880 | 0.9199× | 0.92× | ✅ |
| websocket-64k | 156,130 | 159,694 | 0.9777× | 0.98× | ✅ |
| websocket-fragmented | 12,930 | 16,327 | 0.7919× | 0.79× | ✅ |
| upload | 544,490 | 1,322,380 | 0.4118× | 0.42× | ✅ |
| static | 34,196 | 41,010 | 0.8338× | 0.85× | ✅ |
| pgsql | 23,680 | 24,594 | 0.9628× | 0.96× | ✅ |
| sqlite | 23,911 | 25,232 | 0.9476× | 0.95× | ✅ |
| curl | 85,718 | 90,393 | 0.9483× | 0.95× | ✅ |
| dns | 231,536 | 232,932 | 0.9940× | 1.0× | ✅ |

Geometric mean 0.9047×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
