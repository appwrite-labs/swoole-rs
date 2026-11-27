# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5550ddd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,214 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,050 | 23,170 | 0.9517× | 0.95× | ✅ |
| service-health | 16,308 | 17,782 | 0.9171× | 0.92× | ✅ |
| websocket | 6,740 | 8,192 | 0.8227× | 0.81× | ✅ |
| timer | 202,148 | 313,966 | 0.6439× | 0.65× | ✅ |
| app | 127,545 | 135,989 | 0.9379× | 0.93× | ✅ |
| http2 | 19,173 | 20,497 | 0.9354× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,019 | 155,723 | 0.8799× | 0.88× | ✅ |
| task | 32,072 | 33,272 | 0.9639× | 0.96× | ✅ |
| mysql | 50,286 | 51,088 | 0.9843× | 0.97× | ✅ |
| http-client | 33,661 | 35,783 | 0.9407× | 0.93× | ✅ |
| gzip | 194,278 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,562 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,228 | 0.4954× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9536× | 0.95× | ✅ |
| connect | 17,491 | 18,734 | 0.9337× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,572 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,407 | 17,024 | 0.9050× | 0.89× | ✅ |
| http2-concurrent | 366,931 | 409,220 | 0.8967× | 0.89× | ✅ |
| websocket-64k | 90,037 | 159,691 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,962 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,893 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,457 | 41,092 | 0.6195× | 0.62× | ✅ |
| client | 18,597 | 19,730 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,046 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,884 | 24,667 | 0.9277× | 0.92× | ✅ |
| sqlite | 23,474 | 25,268 | 0.9290× | 0.93× | ✅ |
| curl | 86,083 | 90,418 | 0.9521× | 0.95× | ✅ |
| dns | 230,031 | 232,532 | 0.9892× | 0.99× | ✅ |
| file | 108,880 | 131,207 | 0.8298× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
