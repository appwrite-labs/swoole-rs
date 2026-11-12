# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5e08a1c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,928 | 15,214 | 0.9155× | 0.92× | ✅ |
| echo-1k | 17,050 | 18,165 | 0.9386× | 0.94× | ✅ |
| json | 22,029 | 23,161 | 0.9511× | 0.95× | ✅ |
| service-health | 16,526 | 17,777 | 0.9296× | 0.93× | ✅ |
| websocket | 6,732 | 8,188 | 0.8223× | 0.82× | ✅ |
| timer | 202,870 | 313,647 | 0.6468× | 0.65× | ✅ |
| app | 125,999 | 135,861 | 0.9274× | 0.93× | ✅ |
| http2 | 19,262 | 20,469 | 0.9411× | 0.94× | ✅ |
| table | 195,730 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,876 | 154,654 | 0.8980× | 0.9× | ✅ |
| task | 31,953 | 33,265 | 0.9606× | 0.96× | ✅ |
| mysql | 49,588 | 51,062 | 0.9711× | 0.97× | ✅ |
| http-client | 33,621 | 35,809 | 0.9389× | 0.94× | ✅ |
| client | 18,483 | 19,753 | 0.9357× | 0.94× | ✅ |
| socket | 19,599 | 21,067 | 0.9303× | 0.92× | ✅ |
| gzip | 196,023 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,496 | 181,572 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,025 | 4,765 | 1.0544× | 1.05× | ❌ |
| tcp-eof | 5,567 | 10,228 | 0.5443× | 0.54× | ✅ |
| udp | 7,558 | 7,987 | 0.9463× | 1.0× | ✅ |
| connect | 18,283 | 18,703 | 0.9776× | 0.98× | ✅ |
| https | 108,822 | 108,810 | 1.0001× | 1.0× | ❌ |
| wss | 104,645 | 104,794 | 0.9986× | 1.0× | ✅ |
| atomic | 49,685 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,495 | 16,957 | 0.9728× | 0.97× | ✅ |
| http2-concurrent | 361,656 | 408,881 | 0.8845× | 0.89× | ✅ |
| websocket-64k | 156,181 | 159,691 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 12,887 | 16,327 | 0.7893× | 0.79× | ✅ |
| upload | 478,486 | 1,322,380 | 0.3618× | 0.42× | ✅ |
| static | 29,401 | 41,045 | 0.7163× | 0.84× | ✅ |
| pgsql | 23,224 | 24,594 | 0.9443× | 0.95× | ✅ |
| sqlite | 23,486 | 25,232 | 0.9308× | 0.94× | ✅ |
| curl | 85,277 | 90,328 | 0.9441× | 0.95× | ✅ |
| dns | 230,289 | 232,705 | 0.9896× | 1.0× | ✅ |

Geometric mean 0.8869×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
