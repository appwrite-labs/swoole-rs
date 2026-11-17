# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a48388b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,750 | 15,213 | 0.9038× | 0.91× | ✅ |
| echo-1k | 16,723 | 18,165 | 0.9206× | 0.93× | ✅ |
| json | 22,014 | 23,161 | 0.9505× | 0.95× | ✅ |
| service-health | 16,330 | 17,777 | 0.9186× | 0.92× | ✅ |
| websocket | 6,603 | 8,188 | 0.8064× | 0.8× | ✅ |
| timer | 204,908 | 313,711 | 0.6532× | 0.65× | ✅ |
| app | 126,647 | 135,962 | 0.9315× | 0.93× | ✅ |
| http2 | 18,962 | 20,490 | 0.9254× | 0.92× | ✅ |
| table | 194,337 | 202,900 | 0.9578× | 0.97× | ✅ |
| channel | 137,791 | 155,714 | 0.8849× | 0.88× | ✅ |
| task | 31,877 | 33,268 | 0.9582× | 0.96× | ✅ |
| mysql | 49,954 | 51,064 | 0.9783× | 0.97× | ✅ |
| http-client | 33,606 | 35,804 | 0.9386× | 0.93× | ✅ |
| gzip | 194,198 | 197,093 | 0.9853× | 0.99× | ✅ |
| websocket-deflate | 179,167 | 181,569 | 0.9868× | 0.99× | ✅ |
| tcp-length | 4,565 | 4,765 | 0.9579× | 0.96× | ✅ |
| tcp-eof | 5,074 | 10,231 | 0.4959× | 0.5× | ✅ |
| udp | 7,502 | 7,986 | 0.9394× | 0.93× | ✅ |
| connect | 17,354 | 18,682 | 0.9289× | 0.93× | ✅ |
| https | 105,957 | 108,813 | 0.9738× | 0.98× | ✅ |
| wss | 101,045 | 104,794 | 0.9642× | 0.97× | ✅ |
| atomic | 49,828 | 51,276 | 0.9718× | 0.97× | ✅ |
| process-mode | 15,219 | 16,959 | 0.8974× | 0.89× | ✅ |
| http2-concurrent | 362,005 | 409,073 | 0.8849× | 0.88× | ✅ |
| websocket-64k | 89,869 | 159,693 | 0.5628× | 0.57× | ✅ |
| websocket-fragmented | 11,725 | 16,368 | 0.7163× | 0.72× | ✅ |
| upload | 415,891 | 1,322,413 | 0.3145× | 0.32× | ✅ |
| static | 26,927 | 40,995 | 0.6568× | 0.65× | ✅ |
| client | 18,527 | 19,757 | 0.9377× | 0.94× | ✅ |
| socket | 19,683 | 21,072 | 0.9341× | 0.92× | ✅ |
| pgsql | 22,738 | 24,595 | 0.9245× | 0.93× | ✅ |
| sqlite | 23,448 | 25,236 | 0.9292× | 0.93× | ✅ |
| curl | 86,355 | 90,331 | 0.9560× | 0.94× | ✅ |
| dns | 229,992 | 232,884 | 0.9876× | 0.99× | ✅ |

Geometric mean 0.8516×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
