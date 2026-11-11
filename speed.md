# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `761f237` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,401 | 15,210 | 0.9468× | 0.94× | ✅ |
| echo-1k | 17,530 | 18,168 | 0.9648× | 0.96× | ✅ |
| json | 22,475 | 23,167 | 0.9701× | 0.97× | ✅ |
| service-health | 16,972 | 17,782 | 0.9545× | 0.95× | ✅ |
| websocket | 6,859 | 8,191 | 0.8374× | 0.84× | ✅ |
| timer | 202,854 | 313,629 | 0.6468× | 0.65× | ✅ |
| app | 126,705 | 135,926 | 0.9322× | 0.94× | ✅ |
| http2 | 20,331 | 20,467 | 0.9933× | 0.99× | ✅ |
| table | 196,320 | 201,874 | 0.9725× | 0.98× | ✅ |
| channel | 139,672 | 154,667 | 0.9031× | 0.91× | ✅ |
| task | 32,356 | 33,265 | 0.9727× | 0.97× | ✅ |
| mysql | 49,999 | 51,061 | 0.9792× | 0.98× | ✅ |
| http-client | 34,193 | 35,830 | 0.9543× | 0.96× | ✅ |
| client | 18,888 | 19,765 | 0.9556× | 0.95× | ✅ |
| socket | 20,350 | 21,071 | 0.9658× | 0.97× | ✅ |
| gzip | 196,403 | 197,088 | 0.9965× | 1.0× | ✅ |
| websocket-deflate | 181,633 | 181,568 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,367 | 4,765 | 1.1262× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,982 | 7,987 | 0.9994× | 1.0× | ✅ |
| connect | 19,233 | 18,707 | 1.0281× | 1.03× | ➖ |
| https | 109,615 | 108,810 | 1.0074× | 1.01× | ➖ |
| wss | 105,064 | 104,802 | 1.0025× | 1.0× | ❌ |
| atomic | 50,192 | 51,325 | 0.9779× | 0.98× | ✅ |
| process-mode | 16,653 | 16,957 | 0.9821× | 0.98× | ✅ |
| http2-concurrent | 377,654 | 409,072 | 0.9232× | 0.92× | ✅ |
| websocket-64k | 156,309 | 159,707 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,010 | 16,358 | 0.8564× | 0.87× | ✅ |
| upload | 674,121 | 1,322,488 | 0.5097× | 0.51× | ✅ |
| static | 35,145 | 41,045 | 0.8562× | 0.86× | ✅ |
| pgsql | 23,690 | 24,595 | 0.9632× | 1.03× | ✅ |
| sqlite | 24,003 | 25,228 | 0.9514× | 1.02× | ✅ |
| curl | 86,356 | 90,321 | 0.9561× | 1.01× | ✅ |
| dns | 231,531 | 232,644 | 0.9952× | 1.05× | ✅ |

Geometric mean 0.9201×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
