# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3b863d2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,910 | 15,214 | 0.9143× | 0.92× | ✅ |
| echo-1k | 17,049 | 18,162 | 0.9387× | 0.94× | ✅ |
| json | 22,016 | 23,162 | 0.9505× | 0.95× | ✅ |
| service-health | 16,518 | 17,777 | 0.9292× | 0.93× | ✅ |
| websocket | 6,523 | 8,191 | 0.7964× | 0.81× | ✅ |
| timer | 203,078 | 313,617 | 0.6475× | 0.65× | ✅ |
| app | 125,399 | 135,961 | 0.9223× | 0.93× | ✅ |
| http2 | 18,567 | 20,468 | 0.9071× | 0.92× | ✅ |
| table | 195,492 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,433 | 154,654 | 0.8886× | 0.89× | ✅ |
| task | 31,992 | 33,268 | 0.9616× | 0.96× | ✅ |
| mysql | 49,521 | 51,064 | 0.9698× | 0.97× | ✅ |
| http-client | 33,711 | 35,806 | 0.9415× | 0.94× | ✅ |
| client | 18,469 | 19,757 | 0.9348× | 0.94× | ✅ |
| socket | 19,596 | 21,068 | 0.9302× | 0.92× | ✅ |
| gzip | 194,109 | 197,079 | 0.9849× | 0.99× | ✅ |
| websocket-deflate | 179,354 | 181,568 | 0.9878× | 0.99× | ✅ |
| tcp-length | 4,738 | 4,762 | 0.9949× | 1.01× | ✅ |
| tcp-eof | 5,261 | 10,231 | 0.5142× | 0.52× | ✅ |
| udp | 7,554 | 7,986 | 0.9459× | 0.95× | ✅ |
| connect | 17,849 | 18,707 | 0.9542× | 0.98× | ✅ |
| https | 108,202 | 108,803 | 0.9945× | 1.0× | ✅ |
| wss | 103,791 | 104,800 | 0.9904× | 1.0× | ✅ |
| atomic | 49,631 | 51,316 | 0.9672× | 0.97× | ✅ |
| process-mode | 15,128 | 16,956 | 0.8922× | 0.94× | ✅ |
| http2-concurrent | 357,068 | 408,880 | 0.8733× | 0.88× | ✅ |
| websocket-64k | 155,969 | 159,694 | 0.9767× | 0.98× | ✅ |
| websocket-fragmented | 12,752 | 16,331 | 0.7809× | 0.79× | ✅ |
| upload | 477,849 | 1,322,381 | 0.3614× | 0.37× | ✅ |
| static | 27,616 | 41,016 | 0.6733× | 0.72× | ✅ |
| pgsql | 23,224 | 24,595 | 0.9442× | 0.95× | ✅ |
| sqlite | 23,497 | 25,221 | 0.9316× | 0.94× | ✅ |
| curl | 85,732 | 90,479 | 0.9475× | 0.94× | ✅ |
| dns | 230,735 | 232,801 | 0.9911× | 0.99× | ✅ |

Geometric mean 0.8757×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
