# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c70cf63` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,927 | 15,213 | 0.9155× | 0.92× | ✅ |
| echo-1k | 17,040 | 18,165 | 0.9381× | 0.94× | ✅ |
| json | 22,024 | 23,161 | 0.9509× | 0.95× | ✅ |
| service-health | 16,512 | 17,777 | 0.9289× | 0.93× | ✅ |
| websocket | 6,728 | 8,188 | 0.8217× | 0.82× | ✅ |
| timer | 202,171 | 313,540 | 0.6448× | 0.65× | ✅ |
| app | 125,985 | 135,960 | 0.9266× | 0.93× | ✅ |
| http2 | 19,176 | 20,469 | 0.9368× | 0.94× | ✅ |
| table | 195,720 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 139,383 | 154,654 | 0.9013× | 0.9× | ✅ |
| task | 31,939 | 33,260 | 0.9603× | 0.96× | ✅ |
| mysql | 49,549 | 51,064 | 0.9703× | 0.97× | ✅ |
| http-client | 33,646 | 35,809 | 0.9396× | 0.94× | ✅ |
| client | 18,464 | 19,757 | 0.9346× | 0.94× | ✅ |
| socket | 19,589 | 21,067 | 0.9298× | 0.92× | ✅ |
| gzip | 196,018 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,488 | 181,568 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,023 | 4,762 | 1.0547× | 1.05× | ❌ |
| tcp-eof | 5,565 | 10,231 | 0.5439× | 0.54× | ✅ |
| udp | 7,685 | 7,987 | 0.9622× | 1.0× | ✅ |
| connect | 18,254 | 18,707 | 0.9758× | 0.98× | ✅ |
| https | 108,778 | 108,803 | 0.9998× | 1.0× | ✅ |
| wss | 104,606 | 104,794 | 0.9982× | 1.0× | ✅ |
| atomic | 49,667 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,420 | 17,028 | 0.9643× | 0.97× | ✅ |
| http2-concurrent | 361,357 | 408,877 | 0.8838× | 0.89× | ✅ |
| websocket-64k | 156,179 | 159,692 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 13,083 | 16,331 | 0.8011× | 0.79× | ✅ |
| upload | 544,667 | 1,322,382 | 0.4119× | 0.42× | ✅ |
| static | 34,430 | 41,045 | 0.8388× | 0.84× | ✅ |
| pgsql | 23,193 | 24,594 | 0.9430× | 0.95× | ✅ |
| sqlite | 23,478 | 25,232 | 0.9305× | 0.94× | ✅ |
| curl | 85,308 | 90,511 | 0.9425× | 0.95× | ✅ |
| dns | 231,073 | 232,735 | 0.9929× | 1.0× | ✅ |

Geometric mean 0.8947×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
