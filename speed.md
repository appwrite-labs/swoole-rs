# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ed4e8f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,967 | 15,214 | 0.9181× | 0.92× | ✅ |
| echo-1k | 17,067 | 18,165 | 0.9395× | 0.94× | ✅ |
| json | 22,128 | 23,161 | 0.9554× | 0.95× | ✅ |
| service-health | 16,563 | 17,777 | 0.9317× | 0.93× | ✅ |
| websocket | 6,632 | 8,188 | 0.8100× | 0.8× | ✅ |
| timer | 205,906 | 313,632 | 0.6565× | 0.65× | ✅ |
| app | 125,371 | 135,861 | 0.9228× | 0.93× | ✅ |
| http2 | 18,706 | 20,469 | 0.9139× | 0.91× | ✅ |
| table | 196,030 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 136,083 | 155,171 | 0.8770× | 0.88× | ✅ |
| task | 32,506 | 33,264 | 0.9772× | 0.96× | ✅ |
| mysql | 49,523 | 51,053 | 0.9700× | 0.97× | ✅ |
| http-client | 33,815 | 35,799 | 0.9446× | 0.94× | ✅ |
| client | 18,556 | 19,757 | 0.9392× | 0.94× | ✅ |
| socket | 19,677 | 21,073 | 0.9337× | 0.92× | ✅ |
| gzip | 194,182 | 197,083 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,085 | 181,568 | 0.9863× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9906× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,987 | 0.9443× | 0.93× | ✅ |
| connect | 17,744 | 18,707 | 0.9485× | 0.94× | ✅ |
| https | 108,199 | 108,810 | 0.9944× | 1.0× | ✅ |
| wss | 103,849 | 104,794 | 0.9910× | 0.99× | ✅ |
| atomic | 49,702 | 51,316 | 0.9686× | 0.97× | ✅ |
| process-mode | 15,098 | 16,963 | 0.8901× | 0.88× | ✅ |
| http2-concurrent | 362,150 | 408,877 | 0.8857× | 0.88× | ✅ |
| websocket-64k | 89,960 | 159,692 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,871 | 16,331 | 0.7269× | 0.73× | ✅ |
| upload | 413,023 | 1,322,390 | 0.3123× | 0.32× | ✅ |
| static | 27,249 | 40,981 | 0.6649× | 0.66× | ✅ |
| pgsql | 23,375 | 24,594 | 0.9505× | 0.95× | ✅ |
| sqlite | 23,544 | 25,236 | 0.9329× | 0.93× | ✅ |
| curl | 86,053 | 90,354 | 0.9524× | 0.94× | ✅ |
| dns | 229,756 | 232,563 | 0.9879× | 0.99× | ✅ |

Geometric mean 0.8576×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
