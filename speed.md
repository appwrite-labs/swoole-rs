# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e2c4969` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,897 | 15,213 | 0.9135× | 0.92× | ✅ |
| echo-1k | 17,019 | 18,165 | 0.9369× | 0.94× | ✅ |
| json | 21,987 | 23,161 | 0.9493× | 0.96× | ✅ |
| service-health | 16,487 | 17,777 | 0.9274× | 0.93× | ✅ |
| websocket | 6,728 | 8,188 | 0.8217× | 0.82× | ✅ |
| timer | 202,212 | 313,763 | 0.6445× | 0.65× | ✅ |
| app | 125,977 | 135,950 | 0.9266× | 0.93× | ✅ |
| http2 | 19,182 | 20,464 | 0.9374× | 0.97× | ✅ |
| table | 195,703 | 201,865 | 0.9695× | 0.97× | ✅ |
| channel | 139,363 | 154,651 | 0.9011× | 0.9× | ✅ |
| task | 31,924 | 33,260 | 0.9598× | 0.96× | ✅ |
| mysql | 49,404 | 51,060 | 0.9676× | 0.97× | ✅ |
| http-client | 33,560 | 35,806 | 0.9373× | 0.94× | ✅ |
| client | 18,447 | 19,757 | 0.9337× | 0.94× | ✅ |
| socket | 19,731 | 21,067 | 0.9366× | 0.92× | ✅ |
| gzip | 195,993 | 197,079 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,491 | 181,572 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,023 | 4,762 | 1.0547× | 1.05× | ❌ |
| tcp-eof | 5,565 | 10,228 | 0.5441× | 0.54× | ✅ |
| udp | 8,055 | 7,987 | 1.0085× | 1.0× | ❌ |
| connect | 18,237 | 18,707 | 0.9749× | 0.98× | ✅ |
| https | 108,765 | 108,810 | 0.9996× | 1.0× | ✅ |
| wss | 104,603 | 104,797 | 0.9981× | 1.0× | ✅ |
| atomic | 49,654 | 51,316 | 0.9676× | 0.97× | ✅ |
| process-mode | 16,403 | 17,024 | 0.9635× | 0.97× | ✅ |
| http2-concurrent | 361,233 | 408,880 | 0.8835× | 0.91× | ✅ |
| websocket-64k | 156,171 | 159,694 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,083 | 16,327 | 0.8013× | 0.79× | ✅ |
| upload | 544,622 | 1,322,382 | 0.4118× | 0.42× | ✅ |
| static | 34,463 | 41,074 | 0.8390× | 0.84× | ✅ |
| pgsql | 23,187 | 24,595 | 0.9428× | 0.95× | ✅ |
| sqlite | 23,469 | 25,232 | 0.9301× | 0.94× | ✅ |
| curl | 85,308 | 90,381 | 0.9439× | 0.95× | ✅ |
| dns | 230,848 | 233,241 | 0.9897× | 1.0× | ✅ |

Geometric mean 0.8956×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
