# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `58ccfc7` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,777 | 15,213 | 0.9056× | 0.91× | ✅ |
| echo-1k | 16,791 | 18,165 | 0.9243× | 0.93× | ✅ |
| json | 21,934 | 23,161 | 0.9470× | 0.95× | ✅ |
| service-health | 16,350 | 17,777 | 0.9197× | 0.92× | ✅ |
| websocket | 6,534 | 8,188 | 0.7980× | 0.8× | ✅ |
| timer | 206,183 | 313,630 | 0.6574× | 0.65× | ✅ |
| app | 126,766 | 135,846 | 0.9332× | 0.93× | ✅ |
| http2 | 19,105 | 20,468 | 0.9334× | 0.92× | ✅ |
| table | 194,802 | 201,865 | 0.9650× | 0.97× | ✅ |
| channel | 137,555 | 155,170 | 0.8865× | 0.88× | ✅ |
| task | 32,492 | 33,261 | 0.9769× | 0.96× | ✅ |
| mysql | 49,977 | 51,061 | 0.9788× | 0.97× | ✅ |
| http-client | 33,693 | 35,804 | 0.9411× | 0.93× | ✅ |
| gzip | 194,279 | 197,079 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 178,979 | 181,568 | 0.9857× | 0.99× | ✅ |
| tcp-length | 4,566 | 4,765 | 0.9581× | 0.96× | ✅ |
| tcp-eof | 5,146 | 10,231 | 0.5029× | 0.5× | ✅ |
| udp | 7,468 | 7,986 | 0.9351× | 0.93× | ✅ |
| connect | 17,456 | 18,703 | 0.9333× | 0.93× | ✅ |
| https | 105,936 | 108,810 | 0.9736× | 0.98× | ✅ |
| wss | 100,969 | 104,794 | 0.9635× | 0.97× | ✅ |
| atomic | 49,849 | 51,312 | 0.9715× | 0.97× | ✅ |
| process-mode | 15,009 | 17,024 | 0.8816× | 0.88× | ✅ |
| http2-concurrent | 365,956 | 409,069 | 0.8946× | 0.88× | ✅ |
| websocket-64k | 89,789 | 159,693 | 0.5623× | 0.57× | ✅ |
| websocket-fragmented | 11,601 | 16,331 | 0.7104× | 0.72× | ✅ |
| upload | 415,369 | 1,322,392 | 0.3141× | 0.32× | ✅ |
| static | 26,615 | 41,051 | 0.6483× | 0.65× | ✅ |
| client | 18,577 | 19,756 | 0.9403× | 0.94× | ✅ |
| socket | 19,714 | 21,072 | 0.9356× | 0.92× | ✅ |
| pgsql | 22,764 | 24,595 | 0.9255× | 0.93× | ✅ |
| sqlite | 23,416 | 25,236 | 0.9279× | 0.93× | ✅ |
| curl | 86,323 | 90,351 | 0.9554× | 0.94× | ✅ |
| dns | 229,976 | 233,128 | 0.9865× | 0.99× | ✅ |

Geometric mean 0.8523×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
