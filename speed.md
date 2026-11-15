# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ed4e8f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,971 | 15,213 | 0.9183× | 0.92× | ✅ |
| echo-1k | 17,064 | 18,166 | 0.9393× | 0.94× | ✅ |
| json | 22,128 | 23,161 | 0.9554× | 0.95× | ✅ |
| service-health | 16,563 | 17,777 | 0.9317× | 0.93× | ✅ |
| websocket | 6,633 | 8,188 | 0.8101× | 0.8× | ✅ |
| timer | 205,949 | 313,576 | 0.6568× | 0.65× | ✅ |
| app | 125,372 | 135,886 | 0.9226× | 0.93× | ✅ |
| http2 | 18,708 | 20,469 | 0.9140× | 0.91× | ✅ |
| table | 196,030 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 136,083 | 155,170 | 0.8770× | 0.88× | ✅ |
| task | 32,510 | 33,290 | 0.9766× | 0.96× | ✅ |
| mysql | 49,577 | 51,058 | 0.9710× | 0.97× | ✅ |
| http-client | 33,795 | 35,800 | 0.9440× | 0.94× | ✅ |
| client | 18,538 | 19,754 | 0.9384× | 0.94× | ✅ |
| socket | 19,661 | 21,073 | 0.9330× | 0.92× | ✅ |
| gzip | 194,182 | 197,083 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,094 | 181,564 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9906× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,987 | 0.9444× | 0.93× | ✅ |
| connect | 17,744 | 18,707 | 0.9485× | 0.94× | ✅ |
| https | 108,213 | 108,810 | 0.9945× | 1.0× | ✅ |
| wss | 103,855 | 104,797 | 0.9910× | 0.99× | ✅ |
| atomic | 49,706 | 51,316 | 0.9686× | 0.97× | ✅ |
| process-mode | 15,094 | 16,959 | 0.8901× | 0.88× | ✅ |
| http2-concurrent | 362,014 | 408,880 | 0.8854× | 0.88× | ✅ |
| websocket-64k | 89,957 | 159,696 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,874 | 16,327 | 0.7273× | 0.73× | ✅ |
| upload | 413,026 | 1,322,389 | 0.3123× | 0.32× | ✅ |
| static | 27,284 | 40,987 | 0.6657× | 0.66× | ✅ |
| pgsql | 23,375 | 24,594 | 0.9504× | 0.95× | ✅ |
| sqlite | 23,537 | 25,236 | 0.9327× | 0.93× | ✅ |
| curl | 86,121 | 90,326 | 0.9535× | 0.94× | ✅ |
| dns | 231,026 | 232,738 | 0.9926× | 0.99× | ✅ |

Geometric mean 0.8578×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
