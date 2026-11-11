# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `add9bb2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,446 | 15,213 | 0.9496× | 0.94× | ✅ |
| echo-1k | 17,572 | 18,165 | 0.9673× | 0.97× | ✅ |
| json | 22,537 | 23,170 | 0.9727× | 0.97× | ✅ |
| service-health | 17,027 | 17,786 | 0.9573× | 0.95× | ✅ |
| websocket | 6,889 | 8,188 | 0.8414× | 0.84× | ✅ |
| timer | 202,513 | 313,738 | 0.6455× | 0.65× | ✅ |
| app | 126,705 | 135,900 | 0.9323× | 0.94× | ✅ |
| http2 | 20,370 | 20,464 | 0.9954× | 0.99× | ✅ |
| table | 196,314 | 201,874 | 0.9725× | 0.98× | ✅ |
| channel | 139,730 | 154,668 | 0.9034× | 0.91× | ✅ |
| task | 32,397 | 33,265 | 0.9739× | 0.97× | ✅ |
| mysql | 50,090 | 51,046 | 0.9813× | 0.98× | ✅ |
| http-client | 34,261 | 35,827 | 0.9563× | 0.96× | ✅ |
| client | 18,945 | 19,767 | 0.9584× | 0.95× | ✅ |
| socket | 20,404 | 21,070 | 0.9684× | 0.97× | ✅ |
| gzip | 196,445 | 197,079 | 0.9968× | 1.0× | ✅ |
| websocket-deflate | 181,667 | 181,572 | 1.0005× | 1.0× | ❌ |
| tcp-length | 5,389 | 4,762 | 1.1317× | 1.13× | ❌ |
| tcp-eof | 5,883 | 10,231 | 0.5750× | 0.58× | ✅ |
| udp | 8,014 | 7,987 | 1.0035× | 1.0× | ❌ |
| connect | 19,196 | 18,707 | 1.0262× | 1.03× | ➖ |
| https | 109,620 | 108,813 | 1.0074× | 1.01× | ➖ |
| wss | 105,073 | 104,808 | 1.0025× | 1.0× | ❌ |
| atomic | 50,250 | 51,325 | 0.9791× | 0.98× | ✅ |
| process-mode | 16,701 | 16,956 | 0.9849× | 0.98× | ✅ |
| http2-concurrent | 378,549 | 409,174 | 0.9252× | 0.92× | ✅ |
| websocket-64k | 156,333 | 159,704 | 0.9789× | 0.98× | ✅ |
| websocket-fragmented | 12,840 | 16,339 | 0.7858× | 0.79× | ✅ |
| upload | 675,583 | 1,322,385 | 0.5109× | 0.51× | ✅ |
| static | 34,966 | 41,074 | 0.8513× | 0.86× | ✅ |
| pgsql | 23,728 | 24,594 | 0.9648× | 0.97× | ✅ |
| sqlite | 24,035 | 25,232 | 0.9526× | 0.96× | ✅ |
| curl | 86,391 | 90,490 | 0.9547× | 0.96× | ✅ |
| dns | 231,987 | 233,082 | 0.9953× | 1.0× | ✅ |

Geometric mean 0.9190×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
