# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `aaee64a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,928 | 15,213 | 0.9155× | 0.92× | ✅ |
| echo-1k | 17,072 | 18,165 | 0.9398× | 0.94× | ✅ |
| json | 22,022 | 23,161 | 0.9508× | 0.96× | ✅ |
| service-health | 16,516 | 17,777 | 0.9291× | 0.93× | ✅ |
| websocket | 6,715 | 8,188 | 0.8202× | 0.82× | ✅ |
| timer | 202,548 | 313,549 | 0.6460× | 0.65× | ✅ |
| app | 125,899 | 135,926 | 0.9262× | 0.93× | ✅ |
| http2 | 19,812 | 20,465 | 0.9681× | 0.97× | ✅ |
| table | 195,729 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,876 | 154,654 | 0.8980× | 0.9× | ✅ |
| task | 31,972 | 33,265 | 0.9611× | 0.96× | ✅ |
| mysql | 49,573 | 51,055 | 0.9710× | 0.97× | ✅ |
| http-client | 33,616 | 35,809 | 0.9387× | 0.94× | ✅ |
| client | 18,473 | 19,753 | 0.9352× | 0.94× | ✅ |
| socket | 19,753 | 21,067 | 0.9376× | 0.92× | ✅ |
| gzip | 196,003 | 197,079 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,477 | 181,572 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,023 | 4,765 | 1.0540× | 1.05× | ❌ |
| tcp-eof | 5,565 | 10,228 | 0.5441× | 0.54× | ✅ |
| udp | 8,038 | 7,986 | 1.0064× | 1.0× | ❌ |
| connect | 18,251 | 18,703 | 0.9758× | 0.98× | ✅ |
| https | 108,836 | 108,810 | 1.0002× | 1.01× | ➖ |
| wss | 104,683 | 104,794 | 0.9989× | 1.0× | ✅ |
| atomic | 49,680 | 51,312 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,430 | 16,959 | 0.9688× | 0.97× | ✅ |
| http2-concurrent | 372,297 | 408,880 | 0.9105× | 0.91× | ✅ |
| websocket-64k | 156,162 | 159,697 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,079 | 16,327 | 0.8011× | 0.79× | ✅ |
| upload | 544,890 | 1,322,382 | 0.4121× | 0.42× | ✅ |
| static | 34,448 | 41,045 | 0.8393× | 0.84× | ✅ |
| pgsql | 23,211 | 24,595 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,490 | 25,232 | 0.9309× | 0.94× | ✅ |
| curl | 85,328 | 90,498 | 0.9429× | 0.95× | ✅ |
| dns | 231,067 | 232,682 | 0.9931× | 1.0× | ✅ |

Geometric mean 0.8979×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
