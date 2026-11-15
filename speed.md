# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ed4e8f4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,971 | 15,210 | 0.9186× | 0.92× | ✅ |
| echo-1k | 17,067 | 18,165 | 0.9395× | 0.94× | ✅ |
| json | 22,124 | 23,161 | 0.9552× | 0.95× | ✅ |
| service-health | 16,563 | 17,777 | 0.9317× | 0.93× | ✅ |
| websocket | 6,633 | 8,191 | 0.8097× | 0.8× | ✅ |
| timer | 205,960 | 313,670 | 0.6566× | 0.65× | ✅ |
| app | 125,361 | 135,916 | 0.9223× | 0.93× | ✅ |
| http2 | 18,696 | 20,469 | 0.9134× | 0.91× | ✅ |
| table | 196,026 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 136,083 | 155,170 | 0.8770× | 0.88× | ✅ |
| task | 32,509 | 33,265 | 0.9773× | 0.96× | ✅ |
| mysql | 49,567 | 51,064 | 0.9707× | 0.97× | ✅ |
| http-client | 33,812 | 35,803 | 0.9444× | 0.94× | ✅ |
| client | 18,556 | 19,758 | 0.9392× | 0.94× | ✅ |
| socket | 19,677 | 21,072 | 0.9338× | 0.92× | ✅ |
| gzip | 194,170 | 197,083 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,094 | 181,572 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9906× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,231 | 0.5092× | 0.51× | ✅ |
| udp | 7,542 | 7,987 | 0.9442× | 0.93× | ✅ |
| connect | 17,744 | 18,707 | 0.9485× | 0.94× | ✅ |
| https | 108,199 | 108,810 | 0.9944× | 1.0× | ✅ |
| wss | 103,855 | 104,801 | 0.9910× | 0.99× | ✅ |
| atomic | 49,706 | 51,312 | 0.9687× | 0.97× | ✅ |
| process-mode | 15,098 | 16,963 | 0.8900× | 0.88× | ✅ |
| http2-concurrent | 361,754 | 408,877 | 0.8847× | 0.88× | ✅ |
| websocket-64k | 89,955 | 159,695 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,871 | 16,331 | 0.7269× | 0.73× | ✅ |
| upload | 413,024 | 1,322,387 | 0.3123× | 0.32× | ✅ |
| static | 27,313 | 41,045 | 0.6654× | 0.66× | ✅ |
| pgsql | 23,373 | 24,595 | 0.9503× | 0.95× | ✅ |
| sqlite | 23,537 | 25,236 | 0.9327× | 0.93× | ✅ |
| curl | 86,058 | 90,354 | 0.9524× | 0.94× | ✅ |
| dns | 231,639 | 232,776 | 0.9951× | 0.99× | ✅ |

Geometric mean 0.8578×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
