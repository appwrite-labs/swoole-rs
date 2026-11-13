# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `1ad02cb` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,951 | 15,214 | 0.9170× | 0.92× | ✅ |
| echo-1k | 17,054 | 18,165 | 0.9388× | 0.94× | ✅ |
| json | 22,036 | 23,161 | 0.9514× | 0.95× | ✅ |
| service-health | 16,530 | 17,777 | 0.9299× | 0.93× | ✅ |
| websocket | 6,558 | 8,188 | 0.8009× | 0.81× | ✅ |
| timer | 203,168 | 313,591 | 0.6479× | 0.65× | ✅ |
| app | 125,391 | 135,946 | 0.9224× | 0.93× | ✅ |
| http2 | 18,616 | 20,469 | 0.9095× | 0.92× | ✅ |
| table | 195,504 | 201,865 | 0.9685× | 0.97× | ✅ |
| channel | 137,450 | 154,654 | 0.8888× | 0.89× | ✅ |
| task | 32,002 | 33,268 | 0.9619× | 0.96× | ✅ |
| mysql | 49,554 | 51,061 | 0.9705× | 0.97× | ✅ |
| http-client | 33,691 | 35,809 | 0.9409× | 0.94× | ✅ |
| client | 18,485 | 19,757 | 0.9356× | 0.94× | ✅ |
| socket | 19,613 | 21,063 | 0.9312× | 0.92× | ✅ |
| gzip | 194,153 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,346 | 181,572 | 0.9877× | 0.99× | ✅ |
| tcp-length | 4,772 | 4,762 | 1.0021× | 1.01× | ➖ |
| tcp-eof | 5,295 | 10,231 | 0.5175× | 0.52× | ✅ |
| udp | 7,573 | 7,987 | 0.9482× | 0.95× | ✅ |
| connect | 17,913 | 18,707 | 0.9575× | 0.98× | ✅ |
| https | 108,695 | 108,810 | 0.9989× | 1.0× | ✅ |
| wss | 104,276 | 104,800 | 0.9950× | 1.0× | ✅ |
| atomic | 49,715 | 51,316 | 0.9688× | 0.97× | ✅ |
| process-mode | 15,639 | 16,962 | 0.9220× | 0.94× | ✅ |
| http2-concurrent | 357,547 | 409,561 | 0.8730× | 0.88× | ✅ |
| websocket-64k | 156,005 | 159,694 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,787 | 16,331 | 0.7830× | 0.79× | ✅ |
| upload | 477,906 | 1,322,382 | 0.3614× | 0.37× | ✅ |
| static | 28,685 | 41,045 | 0.6989× | 0.72× | ✅ |
| pgsql | 23,251 | 24,594 | 0.9454× | 0.95× | ✅ |
| sqlite | 23,520 | 25,221 | 0.9325× | 0.94× | ✅ |
| curl | 85,752 | 90,377 | 0.9488× | 0.94× | ✅ |
| dns | 231,111 | 232,738 | 0.9930× | 0.99× | ✅ |

Geometric mean 0.8790×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
