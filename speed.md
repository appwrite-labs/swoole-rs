# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `29a72cd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,878 | 15,214 | 0.9122× | 0.91× | ✅ |
| echo-1k | 16,912 | 18,165 | 0.9310× | 0.93× | ✅ |
| json | 22,041 | 23,161 | 0.9516× | 0.95× | ✅ |
| service-health | 16,471 | 17,777 | 0.9266× | 0.93× | ✅ |
| websocket | 6,509 | 8,188 | 0.7950× | 0.8× | ✅ |
| timer | 206,578 | 313,552 | 0.6588× | 0.65× | ✅ |
| app | 126,005 | 135,886 | 0.9273× | 0.93× | ✅ |
| http2 | 18,846 | 20,468 | 0.9207× | 0.92× | ✅ |
| table | 194,935 | 201,865 | 0.9657× | 0.97× | ✅ |
| channel | 137,932 | 155,170 | 0.8889× | 0.88× | ✅ |
| task | 32,550 | 33,261 | 0.9786× | 0.96× | ✅ |
| mysql | 49,658 | 51,061 | 0.9725× | 0.97× | ✅ |
| http-client | 33,526 | 35,799 | 0.9365× | 0.93× | ✅ |
| gzip | 193,984 | 197,079 | 0.9843× | 0.99× | ✅ |
| websocket-deflate | 178,707 | 181,568 | 0.9842× | 0.99× | ✅ |
| tcp-length | 4,529 | 4,765 | 0.9504× | 0.96× | ✅ |
| tcp-eof | 5,117 | 10,231 | 0.5001× | 0.5× | ✅ |
| udp | 7,439 | 7,987 | 0.9314× | 0.93× | ✅ |
| connect | 17,441 | 18,707 | 0.9323× | 0.93× | ✅ |
| https | 105,929 | 108,813 | 0.9735× | 0.98× | ✅ |
| wss | 100,770 | 104,797 | 0.9616× | 0.97× | ✅ |
| atomic | 49,982 | 51,316 | 0.9740× | 0.97× | ✅ |
| process-mode | 15,071 | 16,957 | 0.8888× | 0.88× | ✅ |
| http2-concurrent | 361,030 | 408,881 | 0.8830× | 0.88× | ✅ |
| websocket-64k | 89,765 | 159,699 | 0.5621× | 0.57× | ✅ |
| websocket-fragmented | 11,671 | 16,327 | 0.7148× | 0.72× | ✅ |
| upload | 414,004 | 1,322,387 | 0.3131× | 0.32× | ✅ |
| static | 26,919 | 40,981 | 0.6569× | 0.65× | ✅ |
| client | 18,556 | 19,757 | 0.9392× | 0.94× | ✅ |
| socket | 19,718 | 21,072 | 0.9357× | 0.92× | ✅ |
| pgsql | 23,042 | 24,594 | 0.9369× | 0.93× | ✅ |
| sqlite | 23,528 | 25,232 | 0.9325× | 0.93× | ✅ |
| curl | 84,897 | 90,351 | 0.9396× | 0.94× | ✅ |
| dns | 229,148 | 232,885 | 0.9840× | 0.99× | ✅ |

Geometric mean 0.8519×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
