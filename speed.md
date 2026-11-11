# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `49972c5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,397 | 15,214 | 0.9463× | 0.94× | ✅ |
| echo-1k | 17,643 | 18,165 | 0.9713× | 0.97× | ✅ |
| json | 22,479 | 23,170 | 0.9702× | 0.97× | ✅ |
| service-health | 16,983 | 17,786 | 0.9549× | 0.95× | ✅ |
| websocket | 6,864 | 8,188 | 0.8383× | 0.84× | ✅ |
| timer | 202,806 | 313,552 | 0.6468× | 0.65× | ✅ |
| app | 126,806 | 135,950 | 0.9327× | 0.94× | ✅ |
| http2 | 20,339 | 20,468 | 0.9937× | 0.99× | ✅ |
| table | 196,330 | 201,874 | 0.9725× | 0.98× | ✅ |
| channel | 139,690 | 154,667 | 0.9032× | 0.91× | ✅ |
| task | 32,469 | 33,261 | 0.9762× | 0.97× | ✅ |
| mysql | 50,019 | 51,061 | 0.9796× | 0.98× | ✅ |
| http-client | 34,202 | 35,830 | 0.9546× | 0.96× | ✅ |
| client | 18,901 | 19,766 | 0.9562× | 0.95× | ✅ |
| socket | 20,360 | 21,071 | 0.9663× | 0.97× | ✅ |
| gzip | 196,518 | 197,079 | 0.9972× | 1.0× | ✅ |
| websocket-deflate | 181,645 | 181,568 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,765 | 1.1261× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,981 | 7,986 | 0.9994× | 1.0× | ✅ |
| connect | 19,253 | 18,703 | 1.0294× | 1.03× | ➖ |
| https | 109,617 | 108,810 | 1.0074× | 1.01× | ➖ |
| wss | 105,073 | 104,802 | 1.0026× | 1.0× | ❌ |
| atomic | 50,206 | 51,325 | 0.9782× | 0.98× | ✅ |
| process-mode | 16,665 | 16,957 | 0.9828× | 0.98× | ✅ |
| http2-concurrent | 378,030 | 409,074 | 0.9241× | 0.92× | ✅ |
| websocket-64k | 156,313 | 159,704 | 0.9788× | 0.98× | ✅ |
| websocket-fragmented | 12,840 | 16,335 | 0.7860× | 0.87× | ✅ |
| upload | 674,265 | 1,322,380 | 0.5099× | 0.51× | ✅ |
| static | 35,157 | 41,045 | 0.8565× | 0.86× | ✅ |
| pgsql | 23,694 | 24,595 | 0.9634× | 0.97× | ✅ |
| sqlite | 23,998 | 25,232 | 0.9511× | 0.96× | ✅ |
| curl | 86,359 | 90,499 | 0.9543× | 0.96× | ✅ |
| dns | 231,709 | 233,115 | 0.9940× | 1.0× | ✅ |

Geometric mean 0.9182×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
