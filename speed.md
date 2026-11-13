# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d57b7e0` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,965 | 15,210 | 0.9182× | 0.92× | ✅ |
| echo-1k | 17,071 | 18,162 | 0.9400× | 0.94× | ✅ |
| json | 22,068 | 23,161 | 0.9528× | 0.95× | ✅ |
| service-health | 16,570 | 17,773 | 0.9323× | 0.93× | ✅ |
| websocket | 6,560 | 8,191 | 0.8009× | 0.81× | ✅ |
| timer | 203,098 | 313,695 | 0.6474× | 0.65× | ✅ |
| app | 125,410 | 135,918 | 0.9227× | 0.93× | ✅ |
| http2 | 18,602 | 20,463 | 0.9091× | 0.92× | ✅ |
| table | 195,525 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,467 | 154,651 | 0.8889× | 0.89× | ✅ |
| task | 32,020 | 33,261 | 0.9627× | 0.96× | ✅ |
| mysql | 49,587 | 51,055 | 0.9713× | 0.97× | ✅ |
| http-client | 33,747 | 35,809 | 0.9424× | 0.94× | ✅ |
| client | 18,503 | 19,756 | 0.9366× | 0.94× | ✅ |
| socket | 19,631 | 21,067 | 0.9318× | 0.92× | ✅ |
| gzip | 194,170 | 197,083 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,346 | 181,572 | 0.9877× | 0.99× | ✅ |
| tcp-length | 4,771 | 4,762 | 1.0019× | 1.01× | ➖ |
| tcp-eof | 5,294 | 10,228 | 0.5176× | 0.52× | ✅ |
| udp | 7,570 | 7,986 | 0.9479× | 0.95× | ✅ |
| connect | 17,917 | 18,707 | 0.9578× | 0.98× | ✅ |
| https | 108,724 | 108,810 | 0.9992× | 1.0× | ✅ |
| wss | 104,292 | 104,794 | 0.9952× | 1.0× | ✅ |
| atomic | 49,732 | 51,316 | 0.9691× | 0.97× | ✅ |
| process-mode | 15,516 | 16,959 | 0.9149× | 0.94× | ✅ |
| http2-concurrent | 357,385 | 408,882 | 0.8741× | 0.88× | ✅ |
| websocket-64k | 156,010 | 159,697 | 0.9769× | 0.98× | ✅ |
| websocket-fragmented | 12,786 | 16,331 | 0.7829× | 0.79× | ✅ |
| upload | 477,895 | 1,322,380 | 0.3614× | 0.37× | ✅ |
| static | 28,568 | 41,045 | 0.6960× | 0.72× | ✅ |
| pgsql | 23,222 | 24,595 | 0.9442× | 0.95× | ✅ |
| sqlite | 23,485 | 25,236 | 0.9306× | 0.94× | ✅ |
| curl | 85,721 | 90,479 | 0.9474× | 0.94× | ✅ |
| dns | 230,907 | 232,944 | 0.9913× | 0.99× | ✅ |

Geometric mean 0.8788×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
