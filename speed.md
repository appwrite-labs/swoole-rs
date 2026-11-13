# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d6ff672` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,918 | 15,210 | 0.9151× | 0.92× | ✅ |
| echo-1k | 17,023 | 18,162 | 0.9373× | 0.94× | ✅ |
| json | 22,020 | 23,158 | 0.9509× | 0.95× | ✅ |
| service-health | 16,518 | 17,773 | 0.9294× | 0.93× | ✅ |
| websocket | 6,527 | 8,188 | 0.7972× | 0.81× | ✅ |
| timer | 203,052 | 313,600 | 0.6475× | 0.65× | ✅ |
| app | 125,356 | 135,989 | 0.9218× | 0.93× | ✅ |
| http2 | 18,574 | 20,464 | 0.9076× | 0.92× | ✅ |
| table | 195,492 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,430 | 154,654 | 0.8886× | 0.89× | ✅ |
| task | 31,965 | 33,265 | 0.9609× | 0.96× | ✅ |
| mysql | 49,525 | 51,064 | 0.9698× | 0.97× | ✅ |
| http-client | 33,688 | 35,812 | 0.9407× | 0.94× | ✅ |
| client | 18,469 | 19,756 | 0.9348× | 0.94× | ✅ |
| socket | 19,597 | 21,067 | 0.9302× | 0.92× | ✅ |
| gzip | 194,109 | 197,079 | 0.9849× | 0.99× | ✅ |
| websocket-deflate | 179,354 | 181,568 | 0.9878× | 0.99× | ✅ |
| tcp-length | 4,738 | 4,762 | 0.9949× | 1.01× | ✅ |
| tcp-eof | 5,261 | 10,231 | 0.5142× | 0.52× | ✅ |
| udp | 7,543 | 7,987 | 0.9444× | 0.95× | ✅ |
| connect | 17,849 | 18,703 | 0.9543× | 0.98× | ✅ |
| https | 108,198 | 108,813 | 0.9943× | 1.0× | ✅ |
| wss | 103,801 | 104,797 | 0.9905× | 1.0× | ✅ |
| atomic | 49,635 | 51,316 | 0.9672× | 0.97× | ✅ |
| process-mode | 15,077 | 16,959 | 0.8890× | 0.94× | ✅ |
| http2-concurrent | 356,932 | 408,881 | 0.8729× | 0.88× | ✅ |
| websocket-64k | 155,965 | 159,694 | 0.9766× | 0.98× | ✅ |
| websocket-fragmented | 12,752 | 16,327 | 0.7810× | 0.79× | ✅ |
| upload | 477,849 | 1,322,380 | 0.3614× | 0.37× | ✅ |
| static | 27,616 | 41,016 | 0.6733× | 0.72× | ✅ |
| pgsql | 23,207 | 24,594 | 0.9436× | 0.95× | ✅ |
| sqlite | 23,473 | 25,236 | 0.9301× | 0.94× | ✅ |
| curl | 85,711 | 90,466 | 0.9474× | 0.94× | ✅ |
| dns | 231,190 | 232,855 | 0.9929× | 0.99× | ✅ |

Geometric mean 0.8756×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
