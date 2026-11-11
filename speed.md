# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c95b5bb` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,464 | 15,214 | 0.9508× | 0.94× | ✅ |
| echo-1k | 17,587 | 18,165 | 0.9682× | 0.97× | ✅ |
| json | 22,551 | 23,170 | 0.9733× | 0.97× | ✅ |
| service-health | 17,037 | 17,786 | 0.9579× | 0.95× | ✅ |
| websocket | 6,884 | 8,188 | 0.8408× | 0.84× | ✅ |
| timer | 202,619 | 313,891 | 0.6455× | 0.65× | ✅ |
| app | 126,723 | 135,804 | 0.9331× | 0.94× | ✅ |
| http2 | 20,370 | 20,464 | 0.9954× | 0.99× | ✅ |
| table | 196,324 | 201,874 | 0.9725× | 0.98× | ✅ |
| channel | 139,741 | 154,667 | 0.9035× | 0.91× | ✅ |
| task | 32,428 | 33,265 | 0.9748× | 0.97× | ✅ |
| mysql | 50,036 | 51,064 | 0.9799× | 0.98× | ✅ |
| http-client | 34,270 | 35,826 | 0.9565× | 0.96× | ✅ |
| client | 18,956 | 19,765 | 0.9590× | 0.95× | ✅ |
| socket | 20,418 | 21,070 | 0.9690× | 0.97× | ✅ |
| gzip | 196,464 | 197,079 | 0.9969× | 1.0× | ✅ |
| websocket-deflate | 181,667 | 181,572 | 1.0005× | 1.0× | ❌ |
| tcp-length | 5,389 | 4,765 | 1.1310× | 1.13× | ❌ |
| tcp-eof | 5,883 | 10,231 | 0.5750× | 0.58× | ✅ |
| udp | 8,014 | 7,986 | 1.0035× | 1.0× | ❌ |
| connect | 19,219 | 18,707 | 1.0274× | 1.03× | ➖ |
| https | 109,639 | 108,810 | 1.0076× | 1.01× | ➖ |
| wss | 105,045 | 104,802 | 1.0023× | 1.0× | ❌ |
| atomic | 50,260 | 51,325 | 0.9793× | 0.98× | ✅ |
| process-mode | 16,711 | 16,959 | 0.9854× | 0.98× | ✅ |
| http2-concurrent | 378,242 | 409,069 | 0.9246× | 0.92× | ✅ |
| websocket-64k | 156,334 | 159,701 | 0.9789× | 0.98× | ✅ |
| websocket-fragmented | 12,836 | 16,339 | 0.7856× | 0.79× | ✅ |
| upload | 675,597 | 1,322,380 | 0.5109× | 0.51× | ✅ |
| static | 34,974 | 41,045 | 0.8521× | 0.86× | ✅ |
| pgsql | 23,757 | 24,595 | 0.9659× | 0.97× | ✅ |
| sqlite | 24,056 | 25,217 | 0.9540× | 0.96× | ✅ |
| curl | 86,416 | 90,339 | 0.9566× | 0.96× | ✅ |
| dns | 231,307 | 232,592 | 0.9945× | 1.0× | ✅ |

Geometric mean 0.9192×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
