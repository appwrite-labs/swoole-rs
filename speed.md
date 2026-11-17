# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `dcaa323` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,750 | 15,213 | 0.9038× | 0.91× | ✅ |
| echo-1k | 16,733 | 18,162 | 0.9214× | 0.93× | ✅ |
| json | 22,058 | 23,158 | 0.9525× | 0.95× | ✅ |
| service-health | 16,327 | 17,777 | 0.9184× | 0.92× | ✅ |
| websocket | 6,629 | 8,188 | 0.8096× | 0.8× | ✅ |
| timer | 206,466 | 313,691 | 0.6582× | 0.65× | ✅ |
| app | 126,207 | 135,907 | 0.9286× | 0.93× | ✅ |
| http2 | 19,084 | 20,495 | 0.9312× | 0.92× | ✅ |
| table | 193,858 | 201,876 | 0.9603× | 0.96× | ✅ |
| channel | 139,201 | 155,717 | 0.8939× | 0.88× | ✅ |
| task | 31,952 | 33,268 | 0.9604× | 0.96× | ✅ |
| mysql | 49,837 | 51,064 | 0.9760× | 0.97× | ✅ |
| http-client | 33,534 | 35,797 | 0.9368× | 0.93× | ✅ |
| gzip | 194,217 | 197,093 | 0.9854× | 0.99× | ✅ |
| websocket-deflate | 179,038 | 181,569 | 0.9861× | 0.99× | ✅ |
| tcp-length | 4,599 | 4,762 | 0.9657× | 0.96× | ✅ |
| tcp-eof | 5,097 | 10,231 | 0.4982× | 0.5× | ✅ |
| udp | 7,545 | 7,986 | 0.9448× | 0.95× | ✅ |
| connect | 17,390 | 18,686 | 0.9307× | 0.93× | ✅ |
| https | 105,939 | 108,809 | 0.9736× | 0.98× | ✅ |
| wss | 101,081 | 104,803 | 0.9645× | 0.97× | ✅ |
| atomic | 49,801 | 51,276 | 0.9712× | 0.97× | ✅ |
| process-mode | 15,242 | 16,957 | 0.8989× | 0.89× | ✅ |
| http2-concurrent | 366,646 | 409,069 | 0.8963× | 0.9× | ✅ |
| websocket-64k | 89,908 | 159,691 | 0.5630× | 0.57× | ✅ |
| websocket-fragmented | 11,758 | 16,368 | 0.7184× | 0.72× | ✅ |
| upload | 415,975 | 1,322,413 | 0.3146× | 0.32× | ✅ |
| static | 26,667 | 40,989 | 0.6506× | 0.65× | ✅ |
| client | 18,421 | 19,757 | 0.9324× | 0.94× | ✅ |
| socket | 19,580 | 21,072 | 0.9292× | 0.93× | ✅ |
| pgsql | 22,707 | 24,594 | 0.9233× | 0.93× | ✅ |
| sqlite | 23,425 | 25,236 | 0.9283× | 0.93× | ✅ |
| curl | 86,259 | 90,353 | 0.9547× | 0.95× | ✅ |
| dns | 230,322 | 232,779 | 0.9894× | 0.99× | ✅ |

Geometric mean 0.8527×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
