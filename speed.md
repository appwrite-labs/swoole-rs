# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ad11922` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,393 | 15,214 | 0.9461× | 0.94× | ✅ |
| echo-1k | 17,647 | 18,162 | 0.9717× | 0.97× | ✅ |
| json | 22,482 | 23,167 | 0.9705× | 0.97× | ✅ |
| service-health | 16,986 | 17,786 | 0.9551× | 0.95× | ✅ |
| websocket | 6,868 | 8,188 | 0.8388× | 0.84× | ✅ |
| timer | 202,683 | 313,691 | 0.6461× | 0.65× | ✅ |
| app | 126,802 | 135,947 | 0.9327× | 0.94× | ✅ |
| http2 | 20,337 | 20,464 | 0.9938× | 0.99× | ✅ |
| table | 196,334 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,686 | 154,668 | 0.9031× | 0.91× | ✅ |
| task | 32,470 | 33,268 | 0.9760× | 0.97× | ✅ |
| mysql | 50,018 | 51,041 | 0.9800× | 0.98× | ✅ |
| http-client | 34,202 | 35,830 | 0.9546× | 0.96× | ✅ |
| client | 18,902 | 19,766 | 0.9563× | 0.95× | ✅ |
| socket | 20,364 | 21,071 | 0.9664× | 0.97× | ✅ |
| gzip | 196,521 | 197,083 | 0.9972× | 1.0× | ✅ |
| websocket-deflate | 181,645 | 181,572 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,762 | 1.1269× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,982 | 7,987 | 0.9994× | 1.0× | ✅ |
| connect | 19,253 | 18,707 | 1.0292× | 1.03× | ➖ |
| https | 109,613 | 108,810 | 1.0074× | 1.01× | ➖ |
| wss | 105,073 | 104,808 | 1.0025× | 1.0× | ❌ |
| atomic | 50,206 | 51,325 | 0.9782× | 0.98× | ✅ |
| process-mode | 16,666 | 16,959 | 0.9827× | 0.98× | ✅ |
| http2-concurrent | 377,930 | 409,069 | 0.9239× | 0.92× | ✅ |
| websocket-64k | 156,313 | 159,707 | 0.9788× | 0.98× | ✅ |
| websocket-fragmented | 14,019 | 16,358 | 0.8570× | 0.87× | ✅ |
| upload | 674,247 | 1,322,380 | 0.5099× | 0.51× | ✅ |
| static | 35,232 | 41,132 | 0.8566× | 0.86× | ✅ |
| pgsql | 23,709 | 24,595 | 0.9640× | 0.97× | ✅ |
| sqlite | 23,998 | 25,232 | 0.9511× | 0.96× | ✅ |
| curl | 86,359 | 90,446 | 0.9548× | 0.96× | ✅ |
| dns | 231,381 | 232,967 | 0.9932× | 1.0× | ✅ |

Geometric mean 0.9206×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
