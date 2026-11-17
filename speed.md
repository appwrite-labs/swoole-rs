# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a1bec8b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,773 | 15,210 | 0.9055× | 0.91× | ✅ |
| echo-1k | 16,794 | 18,165 | 0.9245× | 0.93× | ✅ |
| json | 21,944 | 23,161 | 0.9475× | 0.95× | ✅ |
| service-health | 16,350 | 17,777 | 0.9197× | 0.92× | ✅ |
| websocket | 6,534 | 8,188 | 0.7980× | 0.8× | ✅ |
| timer | 205,863 | 313,579 | 0.6565× | 0.65× | ✅ |
| app | 126,746 | 135,905 | 0.9326× | 0.93× | ✅ |
| http2 | 19,118 | 20,468 | 0.9341× | 0.92× | ✅ |
| table | 194,802 | 201,865 | 0.9650× | 0.97× | ✅ |
| channel | 137,555 | 155,170 | 0.8865× | 0.88× | ✅ |
| task | 32,484 | 33,268 | 0.9764× | 0.96× | ✅ |
| mysql | 49,983 | 51,056 | 0.9790× | 0.97× | ✅ |
| http-client | 33,644 | 35,800 | 0.9398× | 0.93× | ✅ |
| gzip | 194,450 | 197,083 | 0.9866× | 0.99× | ✅ |
| websocket-deflate | 178,983 | 181,572 | 0.9857× | 0.99× | ✅ |
| tcp-length | 4,566 | 4,762 | 0.9588× | 0.96× | ✅ |
| tcp-eof | 5,146 | 10,231 | 0.5029× | 0.5× | ✅ |
| udp | 7,467 | 7,987 | 0.9350× | 0.93× | ✅ |
| connect | 17,456 | 18,707 | 0.9331× | 0.93× | ✅ |
| https | 105,945 | 108,810 | 0.9737× | 0.98× | ✅ |
| wss | 100,965 | 104,797 | 0.9634× | 0.97× | ✅ |
| atomic | 49,849 | 51,316 | 0.9714× | 0.97× | ✅ |
| process-mode | 14,998 | 16,956 | 0.8845× | 0.88× | ✅ |
| http2-concurrent | 366,024 | 409,073 | 0.8948× | 0.88× | ✅ |
| websocket-64k | 89,782 | 159,698 | 0.5622× | 0.57× | ✅ |
| websocket-fragmented | 11,601 | 16,328 | 0.7105× | 0.72× | ✅ |
| upload | 415,379 | 1,322,387 | 0.3141× | 0.32× | ✅ |
| static | 26,615 | 41,051 | 0.6483× | 0.65× | ✅ |
| client | 18,567 | 19,757 | 0.9398× | 0.94× | ✅ |
| socket | 19,714 | 21,072 | 0.9355× | 0.92× | ✅ |
| pgsql | 22,775 | 24,595 | 0.9260× | 0.93× | ✅ |
| sqlite | 23,442 | 25,221 | 0.9294× | 0.93× | ✅ |
| curl | 86,334 | 90,350 | 0.9555× | 0.94× | ✅ |
| dns | 230,422 | 233,090 | 0.9886× | 0.99× | ✅ |

Geometric mean 0.8524×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
