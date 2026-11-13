# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `1ce5249` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,865 | 15,214 | 0.9114× | 0.92× | ✅ |
| echo-1k | 17,003 | 18,162 | 0.9362× | 0.94× | ✅ |
| json | 21,962 | 23,161 | 0.9482× | 0.95× | ✅ |
| service-health | 16,462 | 17,777 | 0.9261× | 0.93× | ✅ |
| websocket | 6,479 | 8,188 | 0.7913× | 0.81× | ✅ |
| timer | 202,992 | 313,621 | 0.6473× | 0.65× | ✅ |
| app | 125,317 | 135,944 | 0.9218× | 0.93× | ✅ |
| http2 | 18,479 | 20,467 | 0.9029× | 0.92× | ✅ |
| table | 195,436 | 201,865 | 0.9682× | 0.97× | ✅ |
| channel | 137,374 | 154,654 | 0.8883× | 0.89× | ✅ |
| task | 31,946 | 33,265 | 0.9603× | 0.96× | ✅ |
| mysql | 49,477 | 51,064 | 0.9689× | 0.97× | ✅ |
| http-client | 33,627 | 35,808 | 0.9391× | 0.94× | ✅ |
| client | 18,414 | 19,754 | 0.9322× | 0.94× | ✅ |
| socket | 19,538 | 21,067 | 0.9274× | 0.92× | ✅ |
| gzip | 194,064 | 197,079 | 0.9847× | 0.99× | ✅ |
| websocket-deflate | 179,261 | 181,568 | 0.9873× | 0.99× | ✅ |
| tcp-length | 4,690 | 4,763 | 0.9848× | 1.01× | ✅ |
| tcp-eof | 5,213 | 10,228 | 0.5097× | 0.52× | ✅ |
| udp | 7,375 | 7,986 | 0.9235× | 0.95× | ✅ |
| connect | 17,526 | 18,707 | 0.9369× | 0.98× | ✅ |
| https | 108,039 | 108,810 | 0.9929× | 1.0× | ✅ |
| wss | 103,631 | 104,797 | 0.9889× | 1.0× | ✅ |
| atomic | 49,579 | 51,316 | 0.9662× | 0.97× | ✅ |
| process-mode | 15,043 | 16,960 | 0.8869× | 0.94× | ✅ |
| http2-concurrent | 357,607 | 408,882 | 0.8746× | 0.88× | ✅ |
| websocket-64k | 155,926 | 159,697 | 0.9764× | 0.98× | ✅ |
| websocket-fragmented | 12,708 | 16,327 | 0.7784× | 0.79× | ✅ |
| upload | 477,855 | 1,322,385 | 0.3614× | 0.37× | ✅ |
| static | 27,294 | 41,010 | 0.6655× | 0.72× | ✅ |
| pgsql | 23,150 | 24,594 | 0.9413× | 0.95× | ✅ |
| sqlite | 23,417 | 25,236 | 0.9279× | 0.94× | ✅ |
| curl | 85,647 | 90,476 | 0.9466× | 0.94× | ✅ |
| dns | 230,991 | 233,381 | 0.9898× | 0.99× | ✅ |

Geometric mean 0.8724×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
