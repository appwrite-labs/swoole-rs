# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f521111` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,737 | 15,214 | 0.9029× | 0.91× | ✅ |
| echo-1k | 16,716 | 18,162 | 0.9204× | 0.93× | ✅ |
| json | 22,065 | 23,158 | 0.9528× | 0.95× | ✅ |
| service-health | 16,312 | 17,777 | 0.9176× | 0.92× | ✅ |
| websocket | 6,659 | 8,188 | 0.8132× | 0.8× | ✅ |
| timer | 200,913 | 313,840 | 0.6402× | 0.65× | ✅ |
| app | 126,624 | 135,803 | 0.9324× | 0.93× | ✅ |
| http2 | 18,960 | 20,495 | 0.9251× | 0.92× | ✅ |
| table | 193,773 | 201,876 | 0.9599× | 0.96× | ✅ |
| channel | 138,359 | 155,717 | 0.8885× | 0.88× | ✅ |
| task | 31,945 | 33,265 | 0.9603× | 0.96× | ✅ |
| mysql | 49,762 | 51,061 | 0.9746× | 0.97× | ✅ |
| http-client | 33,567 | 35,804 | 0.9375× | 0.93× | ✅ |
| gzip | 194,335 | 197,093 | 0.9860× | 0.99× | ✅ |
| websocket-deflate | 179,089 | 181,561 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,659 | 4,765 | 0.9777× | 0.96× | ✅ |
| tcp-eof | 5,162 | 10,231 | 0.5045× | 0.5× | ✅ |
| udp | 7,569 | 7,987 | 0.9477× | 0.95× | ✅ |
| connect | 17,336 | 18,686 | 0.9277× | 0.93× | ✅ |
| https | 106,020 | 108,809 | 0.9744× | 0.98× | ✅ |
| wss | 101,214 | 104,794 | 0.9658× | 0.97× | ✅ |
| atomic | 50,178 | 51,276 | 0.9786× | 0.97× | ✅ |
| process-mode | 15,255 | 17,024 | 0.8961× | 0.89× | ✅ |
| http2-concurrent | 365,124 | 408,881 | 0.8930× | 0.89× | ✅ |
| websocket-64k | 89,957 | 159,693 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,834 | 16,368 | 0.7230× | 0.72× | ✅ |
| upload | 416,135 | 1,322,413 | 0.3147× | 0.32× | ✅ |
| static | 26,810 | 40,989 | 0.6541× | 0.65× | ✅ |
| client | 18,356 | 19,756 | 0.9291× | 0.93× | ✅ |
| socket | 19,576 | 21,072 | 0.9290× | 0.93× | ✅ |
| pgsql | 22,570 | 24,596 | 0.9176× | 0.92× | ✅ |
| sqlite | 23,395 | 25,236 | 0.9271× | 0.93× | ✅ |
| curl | 85,379 | 90,355 | 0.9449× | 0.95× | ✅ |
| dns | 229,853 | 232,483 | 0.9887× | 0.99× | ✅ |

Geometric mean 0.8523×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
