# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `6d9ce7a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,393 | 15,210 | 0.9463× | 0.94× | ✅ |
| echo-1k | 17,648 | 18,172 | 0.9711× | 0.97× | ✅ |
| json | 22,482 | 23,167 | 0.9705× | 0.97× | ✅ |
| service-health | 16,986 | 17,783 | 0.9552× | 0.95× | ✅ |
| websocket | 6,864 | 8,188 | 0.8383× | 0.84× | ✅ |
| timer | 202,797 | 313,578 | 0.6467× | 0.65× | ✅ |
| app | 126,800 | 135,801 | 0.9337× | 0.94× | ✅ |
| http2 | 20,326 | 20,468 | 0.9931× | 0.99× | ✅ |
| table | 196,334 | 201,874 | 0.9726× | 0.98× | ✅ |
| channel | 139,690 | 154,667 | 0.9032× | 0.91× | ✅ |
| task | 32,472 | 33,270 | 0.9760× | 0.97× | ✅ |
| mysql | 49,960 | 51,019 | 0.9792× | 0.98× | ✅ |
| http-client | 34,201 | 35,829 | 0.9545× | 0.96× | ✅ |
| client | 18,898 | 19,767 | 0.9561× | 0.95× | ✅ |
| socket | 20,364 | 21,070 | 0.9665× | 0.97× | ✅ |
| gzip | 196,523 | 197,087 | 0.9971× | 1.0× | ✅ |
| websocket-deflate | 181,645 | 181,572 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,762 | 1.1269× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,982 | 7,987 | 0.9995× | 1.0× | ✅ |
| connect | 19,249 | 18,707 | 1.0290× | 1.03× | ➖ |
| https | 109,629 | 108,810 | 1.0075× | 1.01× | ➖ |
| wss | 105,074 | 104,805 | 1.0026× | 1.0× | ❌ |
| atomic | 50,206 | 51,325 | 0.9782× | 0.98× | ✅ |
| process-mode | 16,653 | 16,956 | 0.9821× | 0.98× | ✅ |
| http2-concurrent | 378,061 | 409,073 | 0.9242× | 0.92× | ✅ |
| websocket-64k | 156,308 | 159,707 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,018 | 16,359 | 0.8569× | 0.87× | ✅ |
| upload | 674,262 | 1,322,437 | 0.5099× | 0.51× | ✅ |
| static | 35,145 | 41,045 | 0.8562× | 0.86× | ✅ |
| pgsql | 23,694 | 24,594 | 0.9634× | 0.97× | ✅ |
| sqlite | 24,003 | 25,232 | 0.9513× | 0.96× | ✅ |
| curl | 86,356 | 90,360 | 0.9557× | 0.96× | ✅ |
| dns | 231,438 | 232,844 | 0.9940× | 1.0× | ✅ |

Geometric mean 0.9206×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
