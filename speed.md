# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `21b5ea1` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,776 | 15,210 | 0.9058× | 0.91× | ✅ |
| echo-1k | 16,747 | 18,162 | 0.9221× | 0.93× | ✅ |
| json | 22,073 | 23,158 | 0.9532× | 0.95× | ✅ |
| service-health | 16,341 | 17,773 | 0.9194× | 0.92× | ✅ |
| websocket | 6,681 | 8,191 | 0.8157× | 0.8× | ✅ |
| timer | 206,206 | 313,692 | 0.6574× | 0.65× | ✅ |
| app | 126,218 | 135,947 | 0.9284× | 0.93× | ✅ |
| http2 | 19,099 | 20,494 | 0.9319× | 0.92× | ✅ |
| table | 193,424 | 202,904 | 0.9533× | 0.96× | ✅ |
| channel | 137,660 | 155,717 | 0.8840× | 0.88× | ✅ |
| task | 32,018 | 33,261 | 0.9626× | 0.96× | ✅ |
| mysql | 49,853 | 51,061 | 0.9763× | 0.97× | ✅ |
| http-client | 33,549 | 35,804 | 0.9370× | 0.93× | ✅ |
| gzip | 194,219 | 197,093 | 0.9854× | 0.99× | ✅ |
| websocket-deflate | 179,084 | 181,569 | 0.9863× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,762 | 0.9756× | 0.96× | ✅ |
| tcp-eof | 5,144 | 10,228 | 0.5029× | 0.5× | ✅ |
| udp | 7,589 | 7,986 | 0.9503× | 0.95× | ✅ |
| connect | 17,447 | 18,686 | 0.9337× | 0.93× | ✅ |
| https | 105,954 | 108,802 | 0.9738× | 0.98× | ✅ |
| wss | 101,138 | 104,801 | 0.9651× | 0.97× | ✅ |
| atomic | 49,815 | 51,276 | 0.9715× | 0.97× | ✅ |
| process-mode | 15,261 | 16,959 | 0.8999× | 0.89× | ✅ |
| http2-concurrent | 367,561 | 409,074 | 0.8985× | 0.9× | ✅ |
| websocket-64k | 89,968 | 159,690 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,818 | 16,364 | 0.7222× | 0.72× | ✅ |
| upload | 415,976 | 1,322,417 | 0.3146× | 0.32× | ✅ |
| static | 26,647 | 41,018 | 0.6496× | 0.65× | ✅ |
| client | 18,437 | 19,756 | 0.9332× | 0.94× | ✅ |
| socket | 19,595 | 21,072 | 0.9299× | 0.94× | ✅ |
| pgsql | 22,736 | 24,595 | 0.9244× | 0.93× | ✅ |
| sqlite | 23,455 | 25,236 | 0.9294× | 0.93× | ✅ |
| curl | 86,505 | 90,340 | 0.9576× | 0.95× | ✅ |
| dns | 230,273 | 233,001 | 0.9883× | 0.99× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
