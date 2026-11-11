# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `fcd66b4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,401 | 15,210 | 0.9468× | 0.97× | ✅ |
| echo-1k | 17,530 | 18,165 | 0.9650× | 0.99× | ✅ |
| json | 22,475 | 23,170 | 0.9700× | 0.98× | ✅ |
| service-health | 16,972 | 17,782 | 0.9545× | 0.97× | ✅ |
| websocket | 6,859 | 8,188 | 0.8377× | 0.84× | ✅ |
| timer | 202,704 | 313,700 | 0.6462× | 0.65× | ✅ |
| app | 126,711 | 135,945 | 0.9321× | 0.94× | ✅ |
| http2 | 20,322 | 20,469 | 0.9928× | 0.99× | ✅ |
| table | 196,320 | 201,874 | 0.9725× | 0.98× | ✅ |
| channel | 139,672 | 154,667 | 0.9031× | 0.91× | ✅ |
| task | 32,355 | 33,291 | 0.9719× | 0.99× | ✅ |
| mysql | 50,020 | 51,061 | 0.9796× | 0.99× | ✅ |
| http-client | 34,192 | 35,830 | 0.9543× | 0.97× | ✅ |
| client | 18,888 | 19,765 | 0.9556× | 0.97× | ✅ |
| socket | 20,347 | 21,071 | 0.9656× | 0.98× | ✅ |
| gzip | 196,407 | 197,091 | 0.9965× | 1.0× | ✅ |
| websocket-deflate | 181,633 | 181,568 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,762 | 1.1269× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,982 | 7,986 | 0.9995× | 1.0× | ✅ |
| connect | 19,229 | 18,707 | 1.0279× | 1.05× | ➖ |
| https | 109,615 | 108,813 | 1.0074× | 1.01× | ➖ |
| wss | 105,065 | 104,802 | 1.0025× | 1.0× | ❌ |
| atomic | 50,192 | 51,325 | 0.9779× | 0.99× | ✅ |
| process-mode | 16,652 | 16,959 | 0.9819× | 1.0× | ✅ |
| http2-concurrent | 377,530 | 409,411 | 0.9221× | 0.92× | ✅ |
| websocket-64k | 156,304 | 159,707 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,010 | 16,359 | 0.8564× | 0.87× | ✅ |
| upload | 674,118 | 1,322,435 | 0.5098× | 0.51× | ✅ |
| static | 35,250 | 41,132 | 0.8570× | 0.86× | ✅ |
| pgsql | 23,695 | 24,594 | 0.9634× | 1.03× | ✅ |
| sqlite | 23,998 | 25,232 | 0.9511× | 1.02× | ✅ |
| curl | 86,356 | 90,398 | 0.9553× | 1.01× | ✅ |
| dns | 231,668 | 232,804 | 0.9951× | 1.05× | ✅ |

Geometric mean 0.9201×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
