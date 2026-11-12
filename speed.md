# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `869d462` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,919 | 15,213 | 0.9149× | 0.93× | ✅ |
| echo-1k | 17,032 | 18,165 | 0.9376× | 0.95× | ✅ |
| json | 22,013 | 23,161 | 0.9504× | 0.96× | ✅ |
| service-health | 16,498 | 17,777 | 0.9281× | 0.94× | ✅ |
| websocket | 6,673 | 8,188 | 0.8150× | 0.82× | ✅ |
| timer | 201,982 | 313,500 | 0.6443× | 0.65× | ✅ |
| app | 125,576 | 135,891 | 0.9241× | 0.93× | ✅ |
| http2 | 19,735 | 20,467 | 0.9642× | 0.98× | ✅ |
| table | 195,779 | 201,865 | 0.9698× | 0.97× | ✅ |
| channel | 138,009 | 154,654 | 0.8924× | 0.9× | ✅ |
| task | 31,915 | 33,264 | 0.9594× | 0.97× | ✅ |
| mysql | 49,414 | 51,061 | 0.9677× | 0.98× | ✅ |
| http-client | 33,435 | 35,809 | 0.9337× | 0.95× | ✅ |
| client | 18,396 | 19,757 | 0.9311× | 0.95× | ✅ |
| socket | 19,359 | 21,063 | 0.9191× | 0.94× | ✅ |
| gzip | 195,884 | 197,079 | 0.9939× | 1.0× | ✅ |
| websocket-deflate | 181,431 | 181,572 | 0.9992× | 1.0× | ✅ |
| tcp-length | 4,983 | 4,762 | 1.0463× | 1.09× | ➖ |
| tcp-eof | 5,525 | 10,231 | 0.5400× | 0.56× | ✅ |
| udp | 7,994 | 7,986 | 1.0010× | 1.0× | ❌ |
| connect | 18,207 | 18,707 | 0.9733× | 0.99× | ✅ |
| https | 108,819 | 108,813 | 1.0001× | 1.01× | ➖ |
| wss | 104,611 | 104,800 | 0.9982× | 1.0× | ✅ |
| atomic | 49,666 | 51,316 | 0.9678× | 0.98× | ✅ |
| process-mode | 16,410 | 16,959 | 0.9676× | 0.98× | ✅ |
| http2-concurrent | 372,107 | 408,881 | 0.9101× | 0.92× | ✅ |
| websocket-64k | 156,118 | 159,696 | 0.9776× | 0.98× | ✅ |
| websocket-fragmented | 13,027 | 16,327 | 0.7979× | 0.79× | ✅ |
| upload | 544,674 | 1,322,382 | 0.4119× | 0.42× | ✅ |
| static | 34,477 | 41,103 | 0.8388× | 0.84× | ✅ |
| pgsql | 23,176 | 24,595 | 0.9423× | 0.96× | ✅ |
| sqlite | 23,474 | 25,232 | 0.9303× | 0.95× | ✅ |
| curl | 85,166 | 90,503 | 0.9410× | 0.95× | ✅ |
| dns | 231,252 | 233,121 | 0.9920× | 1.0× | ✅ |

Geometric mean 0.8953×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
