# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2f7349a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,922 | 15,210 | 0.9153× | 0.92× | ✅ |
| echo-1k | 17,028 | 18,162 | 0.9376× | 0.94× | ✅ |
| json | 22,012 | 23,158 | 0.9505× | 0.95× | ✅ |
| service-health | 16,509 | 17,773 | 0.9289× | 0.93× | ✅ |
| websocket | 6,527 | 8,191 | 0.7968× | 0.81× | ✅ |
| timer | 203,090 | 313,583 | 0.6476× | 0.65× | ✅ |
| app | 125,404 | 135,974 | 0.9223× | 0.93× | ✅ |
| http2 | 18,576 | 20,468 | 0.9076× | 0.92× | ✅ |
| table | 195,483 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,421 | 154,654 | 0.8886× | 0.89× | ✅ |
| task | 31,964 | 33,263 | 0.9609× | 0.96× | ✅ |
| mysql | 49,520 | 51,061 | 0.9698× | 0.97× | ✅ |
| http-client | 33,678 | 35,809 | 0.9405× | 0.94× | ✅ |
| client | 18,456 | 19,754 | 0.9343× | 0.94× | ✅ |
| socket | 19,588 | 21,067 | 0.9298× | 0.92× | ✅ |
| gzip | 194,118 | 197,079 | 0.9850× | 0.99× | ✅ |
| websocket-deflate | 179,309 | 181,568 | 0.9876× | 0.99× | ✅ |
| tcp-length | 4,738 | 4,762 | 0.9949× | 1.01× | ✅ |
| tcp-eof | 5,261 | 10,231 | 0.5142× | 0.52× | ✅ |
| udp | 7,554 | 7,986 | 0.9458× | 0.95× | ✅ |
| connect | 17,849 | 18,707 | 0.9541× | 0.98× | ✅ |
| https | 108,187 | 108,803 | 0.9943× | 1.0× | ✅ |
| wss | 103,792 | 104,800 | 0.9904× | 1.0× | ✅ |
| atomic | 49,690 | 51,316 | 0.9683× | 0.97× | ✅ |
| process-mode | 15,132 | 16,959 | 0.8922× | 0.94× | ✅ |
| http2-concurrent | 356,980 | 409,590 | 0.8716× | 0.88× | ✅ |
| websocket-64k | 155,966 | 159,698 | 0.9766× | 0.98× | ✅ |
| websocket-fragmented | 12,752 | 16,331 | 0.7809× | 0.79× | ✅ |
| upload | 477,888 | 1,322,384 | 0.3614× | 0.37× | ✅ |
| static | 27,610 | 41,016 | 0.6731× | 0.72× | ✅ |
| pgsql | 23,195 | 24,594 | 0.9431× | 0.95× | ✅ |
| sqlite | 23,466 | 25,221 | 0.9304× | 0.94× | ✅ |
| curl | 85,695 | 90,472 | 0.9472× | 0.94× | ✅ |
| dns | 230,623 | 232,834 | 0.9905× | 0.99× | ✅ |

Geometric mean 0.8756×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
