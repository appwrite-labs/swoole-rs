# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `860c57f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,474 | 15,213 | 0.9514× | 0.94× | ✅ |
| echo-1k | 17,599 | 18,166 | 0.9688× | 0.97× | ✅ |
| json | 22,563 | 23,170 | 0.9738× | 0.97× | ✅ |
| service-health | 17,053 | 17,786 | 0.9588× | 0.95× | ✅ |
| websocket | 6,913 | 8,184 | 0.8446× | 0.84× | ✅ |
| timer | 202,717 | 313,616 | 0.6464× | 0.65× | ✅ |
| app | 126,735 | 135,937 | 0.9323× | 0.94× | ✅ |
| http2 | 20,410 | 20,468 | 0.9972× | 0.99× | ✅ |
| table | 196,339 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,756 | 154,664 | 0.9036× | 0.91× | ✅ |
| task | 32,440 | 33,266 | 0.9751× | 0.97× | ✅ |
| mysql | 50,092 | 51,064 | 0.9810× | 0.98× | ✅ |
| http-client | 34,307 | 35,826 | 0.9576× | 0.96× | ✅ |
| client | 18,968 | 19,763 | 0.9597× | 0.95× | ✅ |
| socket | 20,433 | 21,070 | 0.9698× | 0.97× | ✅ |
| gzip | 196,457 | 197,079 | 0.9968× | 1.0× | ✅ |
| websocket-deflate | 181,686 | 181,568 | 1.0006× | 1.0× | ❌ |
| tcp-length | 5,441 | 4,762 | 1.1425× | 1.13× | ❌ |
| tcp-eof | 5,934 | 10,231 | 0.5800× | 0.58× | ✅ |
| udp | 8,018 | 7,987 | 1.0039× | 1.0× | ❌ |
| connect | 18,908 | 18,707 | 1.0107× | 1.02× | ➖ |
| https | 109,668 | 108,813 | 1.0079× | 1.01× | ➖ |
| wss | 105,077 | 104,805 | 1.0026× | 1.0× | ❌ |
| atomic | 50,276 | 51,325 | 0.9796× | 0.98× | ✅ |
| process-mode | 16,742 | 16,963 | 0.9870× | 0.98× | ✅ |
| http2-concurrent | 379,668 | 409,078 | 0.9281× | 0.92× | ✅ |
| websocket-64k | 156,358 | 159,705 | 0.9790× | 0.98× | ✅ |
| websocket-fragmented | 12,881 | 16,335 | 0.7885× | 0.79× | ✅ |
| upload | 675,602 | 1,322,385 | 0.5109× | 0.51× | ✅ |
| static | 34,747 | 40,987 | 0.8478× | 0.85× | ✅ |
| pgsql | 23,783 | 24,594 | 0.9670× | 0.97× | ✅ |
| sqlite | 24,072 | 25,232 | 0.9540× | 0.96× | ✅ |
| curl | 86,367 | 90,386 | 0.9555× | 0.96× | ✅ |
| dns | 231,674 | 232,990 | 0.9943× | 1.0× | ✅ |

Geometric mean 0.9198×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
