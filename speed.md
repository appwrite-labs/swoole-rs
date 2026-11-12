# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f479b88` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,923 | 15,210 | 0.9154× | 0.93× | ✅ |
| echo-1k | 17,036 | 18,162 | 0.9380× | 0.95× | ✅ |
| json | 22,017 | 23,161 | 0.9506× | 0.96× | ✅ |
| service-health | 16,502 | 17,773 | 0.9285× | 0.94× | ✅ |
| websocket | 6,670 | 8,191 | 0.8142× | 0.82× | ✅ |
| timer | 201,870 | 313,514 | 0.6439× | 0.65× | ✅ |
| app | 125,573 | 135,884 | 0.9241× | 0.93× | ✅ |
| http2 | 19,724 | 20,463 | 0.9639× | 0.98× | ✅ |
| table | 195,779 | 201,865 | 0.9698× | 0.97× | ✅ |
| channel | 138,012 | 154,651 | 0.8924× | 0.9× | ✅ |
| task | 31,910 | 33,265 | 0.9593× | 0.97× | ✅ |
| mysql | 49,322 | 51,062 | 0.9659× | 0.98× | ✅ |
| http-client | 33,432 | 35,804 | 0.9337× | 0.95× | ✅ |
| client | 18,397 | 19,757 | 0.9312× | 0.95× | ✅ |
| socket | 19,363 | 21,063 | 0.9193× | 0.94× | ✅ |
| gzip | 195,888 | 197,083 | 0.9939× | 1.0× | ✅ |
| websocket-deflate | 181,431 | 181,572 | 0.9992× | 1.0× | ✅ |
| tcp-length | 4,983 | 4,765 | 1.0456× | 1.09× | ➖ |
| tcp-eof | 5,525 | 10,228 | 0.5401× | 0.56× | ✅ |
| udp | 7,994 | 7,986 | 1.0010× | 1.0× | ❌ |
| connect | 18,207 | 18,707 | 0.9733× | 0.99× | ✅ |
| https | 108,819 | 108,810 | 1.0001× | 1.01× | ➖ |
| wss | 104,628 | 104,797 | 0.9984× | 1.0× | ✅ |
| atomic | 49,666 | 51,316 | 0.9678× | 0.98× | ✅ |
| process-mode | 16,410 | 16,957 | 0.9677× | 0.98× | ✅ |
| http2-concurrent | 371,990 | 408,960 | 0.9096× | 0.92× | ✅ |
| websocket-64k | 156,121 | 159,696 | 0.9776× | 0.98× | ✅ |
| websocket-fragmented | 13,023 | 16,331 | 0.7974× | 0.79× | ✅ |
| upload | 544,674 | 1,322,380 | 0.4119× | 0.42× | ✅ |
| static | 34,448 | 41,074 | 0.8387× | 0.84× | ✅ |
| pgsql | 23,181 | 24,594 | 0.9425× | 0.96× | ✅ |
| sqlite | 23,474 | 25,232 | 0.9303× | 0.95× | ✅ |
| curl | 85,167 | 90,503 | 0.9410× | 0.95× | ✅ |
| dns | 230,858 | 232,834 | 0.9915× | 1.0× | ✅ |

Geometric mean 0.8953×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
