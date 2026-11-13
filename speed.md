# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `85bd4e7` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,932 | 15,210 | 0.9160× | 0.92× | ✅ |
| echo-1k | 17,033 | 18,165 | 0.9377× | 0.94× | ✅ |
| json | 22,032 | 23,161 | 0.9512× | 0.95× | ✅ |
| service-health | 16,537 | 17,777 | 0.9302× | 0.93× | ✅ |
| websocket | 6,523 | 8,191 | 0.7964× | 0.81× | ✅ |
| timer | 203,083 | 313,761 | 0.6473× | 0.65× | ✅ |
| app | 125,364 | 135,958 | 0.9221× | 0.93× | ✅ |
| http2 | 18,565 | 20,464 | 0.9072× | 0.92× | ✅ |
| table | 195,492 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,433 | 154,654 | 0.8886× | 0.89× | ✅ |
| task | 31,974 | 33,265 | 0.9612× | 0.96× | ✅ |
| mysql | 49,568 | 51,058 | 0.9708× | 0.97× | ✅ |
| http-client | 33,710 | 35,805 | 0.9415× | 0.94× | ✅ |
| client | 18,467 | 19,758 | 0.9347× | 0.94× | ✅ |
| socket | 19,593 | 21,068 | 0.9300× | 0.92× | ✅ |
| gzip | 194,128 | 197,079 | 0.9850× | 0.99× | ✅ |
| websocket-deflate | 179,357 | 181,572 | 0.9878× | 0.99× | ✅ |
| tcp-length | 4,738 | 4,765 | 0.9942× | 1.01× | ✅ |
| tcp-eof | 5,261 | 10,228 | 0.5143× | 0.52× | ✅ |
| udp | 7,531 | 7,987 | 0.9429× | 0.95× | ✅ |
| connect | 17,711 | 18,707 | 0.9468× | 0.98× | ✅ |
| https | 108,108 | 108,810 | 0.9935× | 1.0× | ✅ |
| wss | 103,674 | 104,800 | 0.9892× | 1.0× | ✅ |
| atomic | 49,635 | 51,312 | 0.9673× | 0.97× | ✅ |
| process-mode | 15,107 | 17,022 | 0.8875× | 0.94× | ✅ |
| http2-concurrent | 356,837 | 408,880 | 0.8727× | 0.88× | ✅ |
| websocket-64k | 155,966 | 159,696 | 0.9766× | 0.98× | ✅ |
| websocket-fragmented | 12,752 | 16,327 | 0.7810× | 0.79× | ✅ |
| upload | 477,872 | 1,322,380 | 0.3614× | 0.37× | ✅ |
| static | 27,335 | 40,987 | 0.6669× | 0.72× | ✅ |
| pgsql | 23,210 | 24,594 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,459 | 25,236 | 0.9296× | 0.94× | ✅ |
| curl | 85,698 | 90,355 | 0.9485× | 0.94× | ✅ |
| dns | 230,759 | 233,050 | 0.9902× | 0.99× | ✅ |

Geometric mean 0.8750×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
