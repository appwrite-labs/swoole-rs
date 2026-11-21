# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a4c8db3` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,754 | 15,214 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,083 | 23,173 | 0.9530× | 0.95× | ✅ |
| service-health | 16,338 | 17,786 | 0.9186× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 202,080 | 313,818 | 0.6439× | 0.65× | ✅ |
| app | 127,612 | 135,912 | 0.9389× | 0.93× | ✅ |
| http2 | 18,969 | 20,499 | 0.9254× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,723 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,265 | 0.9646× | 0.96× | ✅ |
| mysql | 50,308 | 51,085 | 0.9848× | 0.97× | ✅ |
| http-client | 33,661 | 35,792 | 0.9405× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,529 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,599 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,413 | 17,025 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,163 | 409,053 | 0.8903× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,513 | 1,322,430 | 0.3150× | 0.32× | ✅ |
| static | 25,860 | 40,999 | 0.6307× | 0.64× | ✅ |
| client | 18,621 | 19,730 | 0.9438× | 0.93× | ✅ |
| socket | 19,860 | 21,045 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,827 | 24,662 | 0.9256× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,137 | 90,550 | 0.9513× | 0.95× | ✅ |
| dns | 229,941 | 232,744 | 0.9880× | 0.99× | ✅ |
| file | 109,092 | 131,211 | 0.8314× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
