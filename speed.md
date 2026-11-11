# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `dc7161b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,089 | 15,210 | 0.9263× | 0.94× | ✅ |
| echo-1k | 17,191 | 18,165 | 0.9464× | 0.96× | ✅ |
| json | 22,177 | 23,170 | 0.9571× | 0.97× | ✅ |
| service-health | 16,675 | 17,786 | 0.9375× | 0.95× | ✅ |
| websocket | 6,724 | 8,191 | 0.8209× | 0.84× | ✅ |
| timer | 202,054 | 313,652 | 0.6442× | 0.65× | ✅ |
| app | 126,463 | 135,913 | 0.9305× | 0.93× | ✅ |
| http2 | 19,920 | 20,464 | 0.9734× | 0.99× | ✅ |
| table | 195,748 | 201,874 | 0.9697× | 0.98× | ✅ |
| channel | 139,363 | 154,668 | 0.9010× | 0.91× | ✅ |
| task | 31,990 | 33,268 | 0.9616× | 0.97× | ✅ |
| mysql | 49,790 | 51,059 | 0.9752× | 0.98× | ✅ |
| http-client | 33,944 | 35,829 | 0.9474× | 0.95× | ✅ |
| client | 18,575 | 19,765 | 0.9398× | 0.94× | ✅ |
| socket | 19,613 | 21,071 | 0.9308× | 0.94× | ✅ |
| gzip | 196,055 | 197,079 | 0.9948× | 1.0× | ✅ |
| websocket-deflate | 181,485 | 181,568 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,232 | 4,765 | 1.0979× | 1.13× | ➖ |
| tcp-eof | 5,802 | 10,231 | 0.5671× | 0.58× | ✅ |
| udp | 7,989 | 7,986 | 1.0003× | 1.0× | ❌ |
| connect | 18,342 | 18,707 | 0.9805× | 0.99× | ✅ |
| https | 109,192 | 108,810 | 1.0035× | 1.01× | ➖ |
| wss | 104,849 | 104,805 | 1.0004× | 1.0× | ❌ |
| atomic | 49,892 | 51,325 | 0.9721× | 0.98× | ✅ |
| process-mode | 16,458 | 16,959 | 0.9705× | 0.98× | ✅ |
| http2-concurrent | 373,353 | 409,074 | 0.9127× | 0.92× | ✅ |
| websocket-64k | 156,173 | 159,706 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 12,977 | 16,336 | 0.7944× | 0.79× | ✅ |
| upload | 544,268 | 1,322,380 | 0.4116× | 0.42× | ✅ |
| static | 34,556 | 41,010 | 0.8426× | 0.85× | ✅ |
| pgsql | 23,397 | 24,594 | 0.9513× | 0.96× | ✅ |
| sqlite | 23,731 | 25,217 | 0.9411× | 0.95× | ✅ |
| curl | 85,688 | 90,512 | 0.9467× | 0.95× | ✅ |
| dns | 231,562 | 233,704 | 0.9908× | 1.0× | ✅ |

Geometric mean 0.9023×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
