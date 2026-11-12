# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3417f1d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,926 | 15,210 | 0.9156× | 0.92× | ✅ |
| echo-1k | 17,044 | 18,165 | 0.9383× | 0.94× | ✅ |
| json | 22,024 | 23,161 | 0.9509× | 0.95× | ✅ |
| service-health | 16,516 | 17,777 | 0.9291× | 0.93× | ✅ |
| websocket | 6,729 | 8,188 | 0.8218× | 0.82× | ✅ |
| timer | 202,227 | 313,656 | 0.6447× | 0.65× | ✅ |
| app | 125,962 | 135,889 | 0.9269× | 0.93× | ✅ |
| http2 | 19,180 | 20,464 | 0.9372× | 0.94× | ✅ |
| table | 195,720 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,864 | 154,654 | 0.8979× | 0.9× | ✅ |
| task | 31,935 | 33,264 | 0.9600× | 0.96× | ✅ |
| mysql | 49,563 | 51,055 | 0.9708× | 0.97× | ✅ |
| http-client | 33,604 | 35,809 | 0.9384× | 0.94× | ✅ |
| client | 18,468 | 19,756 | 0.9348× | 0.94× | ✅ |
| socket | 19,583 | 21,063 | 0.9297× | 0.92× | ✅ |
| gzip | 196,022 | 197,083 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,492 | 181,572 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,023 | 4,765 | 1.0540× | 1.05× | ❌ |
| tcp-eof | 5,565 | 10,231 | 0.5439× | 0.54× | ✅ |
| udp | 7,559 | 7,986 | 0.9466× | 1.0× | ✅ |
| connect | 18,254 | 18,707 | 0.9758× | 0.98× | ✅ |
| https | 108,806 | 108,813 | 0.9999× | 1.0× | ✅ |
| wss | 104,636 | 104,794 | 0.9985× | 1.0× | ✅ |
| atomic | 49,671 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,413 | 16,956 | 0.9680× | 0.97× | ✅ |
| http2-concurrent | 361,125 | 408,880 | 0.8832× | 0.89× | ✅ |
| websocket-64k | 156,178 | 159,694 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 13,088 | 16,327 | 0.8016× | 0.79× | ✅ |
| upload | 544,677 | 1,322,380 | 0.4119× | 0.42× | ✅ |
| static | 34,459 | 41,045 | 0.8395× | 0.84× | ✅ |
| pgsql | 23,199 | 24,595 | 0.9433× | 0.95× | ✅ |
| sqlite | 23,473 | 25,232 | 0.9303× | 0.94× | ✅ |
| curl | 85,314 | 90,381 | 0.9439× | 0.95× | ✅ |
| dns | 229,883 | 232,787 | 0.9875× | 1.0× | ✅ |

Geometric mean 0.8942×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
