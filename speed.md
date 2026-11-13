# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `fa4f71b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,920 | 15,214 | 0.9150× | 0.92× | ✅ |
| echo-1k | 17,022 | 18,162 | 0.9372× | 0.94× | ✅ |
| json | 22,026 | 23,158 | 0.9511× | 0.95× | ✅ |
| service-health | 16,528 | 17,773 | 0.9299× | 0.93× | ✅ |
| websocket | 6,581 | 8,191 | 0.8034× | 0.82× | ✅ |
| timer | 203,380 | 313,520 | 0.6487× | 0.65× | ✅ |
| app | 125,769 | 135,942 | 0.9252× | 0.93× | ✅ |
| http2 | 18,645 | 20,464 | 0.9111× | 0.93× | ✅ |
| table | 195,488 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,423 | 154,651 | 0.8886× | 0.9× | ✅ |
| task | 32,096 | 33,265 | 0.9649× | 0.96× | ✅ |
| mysql | 49,520 | 51,052 | 0.9700× | 0.97× | ✅ |
| http-client | 33,759 | 35,803 | 0.9429× | 0.94× | ✅ |
| client | 18,483 | 19,757 | 0.9355× | 0.94× | ✅ |
| socket | 19,599 | 21,067 | 0.9303× | 0.92× | ✅ |
| gzip | 194,117 | 197,083 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,383 | 181,572 | 0.9879× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,765 | 1.0055× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,228 | 0.5221× | 0.54× | ✅ |
| udp | 7,581 | 7,987 | 0.9491× | 0.95× | ✅ |
| connect | 18,331 | 18,707 | 0.9799× | 0.98× | ✅ |
| https | 108,796 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,472 | 104,800 | 0.9969× | 1.0× | ✅ |
| atomic | 49,684 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,674 | 16,963 | 0.9830× | 0.99× | ✅ |
| http2-concurrent | 358,985 | 408,877 | 0.8780× | 0.88× | ✅ |
| websocket-64k | 156,022 | 159,697 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,812 | 16,331 | 0.7846× | 0.79× | ✅ |
| upload | 477,826 | 1,322,383 | 0.3613× | 0.37× | ✅ |
| static | 29,293 | 41,074 | 0.7132× | 0.72× | ✅ |
| pgsql | 23,200 | 24,595 | 0.9433× | 0.95× | ✅ |
| sqlite | 23,478 | 25,236 | 0.9303× | 0.94× | ✅ |
| curl | 84,750 | 90,355 | 0.9380× | 0.94× | ✅ |
| dns | 230,622 | 233,013 | 0.9897× | 0.99× | ✅ |

Geometric mean 0.8821×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
