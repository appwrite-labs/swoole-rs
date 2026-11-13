# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5fcab99` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,944 | 15,214 | 0.9165× | 0.92× | ✅ |
| echo-1k | 17,049 | 18,166 | 0.9385× | 0.94× | ✅ |
| json | 22,119 | 23,161 | 0.9550× | 0.95× | ✅ |
| service-health | 16,569 | 17,777 | 0.9321× | 0.93× | ✅ |
| websocket | 6,558 | 8,188 | 0.8010× | 0.8× | ✅ |
| timer | 204,821 | 313,536 | 0.6533× | 0.65× | ✅ |
| app | 125,569 | 135,867 | 0.9242× | 0.93× | ✅ |
| http2 | 18,670 | 20,468 | 0.9121× | 0.91× | ✅ |
| table | 196,026 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 137,521 | 155,170 | 0.8863× | 0.89× | ✅ |
| task | 32,006 | 33,268 | 0.9621× | 0.96× | ✅ |
| mysql | 49,660 | 51,052 | 0.9727× | 0.97× | ✅ |
| http-client | 33,859 | 35,800 | 0.9458× | 0.94× | ✅ |
| client | 18,597 | 19,756 | 0.9413× | 0.94× | ✅ |
| socket | 19,729 | 21,072 | 0.9362× | 0.92× | ✅ |
| gzip | 194,164 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,040 | 181,560 | 0.9861× | 0.99× | ✅ |
| tcp-length | 4,708 | 4,765 | 0.9880× | 0.99× | ✅ |
| tcp-eof | 5,201 | 10,231 | 0.5083× | 0.51× | ✅ |
| udp | 7,374 | 7,986 | 0.9234× | 0.93× | ✅ |
| connect | 17,706 | 18,707 | 0.9465× | 0.94× | ✅ |
| https | 108,202 | 108,810 | 0.9944× | 1.0× | ✅ |
| wss | 103,781 | 104,794 | 0.9903× | 0.99× | ✅ |
| atomic | 49,721 | 51,316 | 0.9689× | 0.97× | ✅ |
| process-mode | 14,994 | 16,963 | 0.8839× | 0.88× | ✅ |
| http2-concurrent | 359,511 | 409,008 | 0.8790× | 0.88× | ✅ |
| websocket-64k | 89,897 | 159,693 | 0.5629× | 0.98× | ✅ |
| websocket-fragmented | 12,016 | 16,331 | 0.7358× | 0.78× | ✅ |
| upload | 412,404 | 1,322,391 | 0.3119× | 0.37× | ✅ |
| static | 27,027 | 41,003 | 0.6591× | 0.67× | ✅ |
| pgsql | 23,385 | 24,594 | 0.9508× | 0.95× | ✅ |
| sqlite | 23,562 | 25,236 | 0.9337× | 0.93× | ✅ |
| curl | 85,748 | 90,479 | 0.9477× | 0.94× | ✅ |
| dns | 230,426 | 232,805 | 0.9898× | 0.99× | ✅ |

Geometric mean 0.8561×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
