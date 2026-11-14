# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2080277` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,953 | 15,214 | 0.9171× | 0.92× | ✅ |
| echo-1k | 17,050 | 18,165 | 0.9386× | 0.94× | ✅ |
| json | 22,094 | 23,161 | 0.9539× | 0.95× | ✅ |
| service-health | 16,539 | 17,777 | 0.9304× | 0.93× | ✅ |
| websocket | 6,632 | 8,188 | 0.8100× | 0.8× | ✅ |
| timer | 205,874 | 313,569 | 0.6566× | 0.65× | ✅ |
| app | 125,549 | 135,824 | 0.9244× | 0.93× | ✅ |
| http2 | 18,672 | 20,464 | 0.9124× | 0.91× | ✅ |
| table | 195,997 | 201,865 | 0.9709× | 0.97× | ✅ |
| channel | 136,690 | 155,170 | 0.8809× | 0.89× | ✅ |
| task | 32,496 | 33,268 | 0.9768× | 0.96× | ✅ |
| mysql | 49,700 | 51,061 | 0.9734× | 0.97× | ✅ |
| http-client | 33,895 | 35,798 | 0.9468× | 0.94× | ✅ |
| client | 18,585 | 19,754 | 0.9408× | 0.94× | ✅ |
| socket | 19,705 | 21,073 | 0.9351× | 0.92× | ✅ |
| gzip | 194,162 | 197,080 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,572 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,717 | 4,762 | 0.9905× | 0.99× | ✅ |
| tcp-eof | 5,210 | 10,228 | 0.5094× | 0.51× | ✅ |
| udp | 7,508 | 7,986 | 0.9401× | 0.93× | ✅ |
| connect | 17,704 | 18,707 | 0.9464× | 0.94× | ✅ |
| https | 108,196 | 108,810 | 0.9944× | 1.0× | ✅ |
| wss | 103,854 | 104,803 | 0.9909× | 0.99× | ✅ |
| atomic | 49,692 | 51,316 | 0.9684× | 0.97× | ✅ |
| process-mode | 15,100 | 17,024 | 0.8870× | 0.88× | ✅ |
| http2-concurrent | 361,133 | 409,072 | 0.8828× | 0.88× | ✅ |
| websocket-64k | 89,959 | 159,696 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 12,078 | 16,327 | 0.7397× | 0.74× | ✅ |
| upload | 413,349 | 1,322,387 | 0.3126× | 0.32× | ✅ |
| static | 27,279 | 40,993 | 0.6655× | 0.66× | ✅ |
| pgsql | 23,345 | 24,595 | 0.9492× | 0.95× | ✅ |
| sqlite | 23,538 | 25,236 | 0.9327× | 0.93× | ✅ |
| curl | 85,896 | 90,351 | 0.9507× | 0.94× | ✅ |
| dns | 231,403 | 232,927 | 0.9935× | 0.99× | ✅ |

Geometric mean 0.8580×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
