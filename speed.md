# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `97ce799` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,771 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,165 | 0.9256× | 0.92× | ✅ |
| json | 22,104 | 23,173 | 0.9539× | 0.95× | ✅ |
| service-health | 16,362 | 17,782 | 0.9201× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 202,015 | 313,820 | 0.6437× | 0.65× | ✅ |
| app | 127,633 | 135,913 | 0.9391× | 0.93× | ✅ |
| http2 | 18,959 | 20,497 | 0.9250× | 0.92× | ✅ |
| table | 194,327 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,383 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,092 | 33,268 | 0.9646× | 0.96× | ✅ |
| mysql | 50,321 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,681 | 35,797 | 0.9409× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,542 | 18,734 | 0.9364× | 0.93× | ✅ |
| https | 106,331 | 108,809 | 0.9772× | 0.98× | ✅ |
| wss | 101,599 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,217 | 51,281 | 0.9793× | 0.97× | ✅ |
| process-mode | 15,438 | 17,021 | 0.9070× | 0.89× | ✅ |
| http2-concurrent | 363,551 | 409,057 | 0.8888× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,372 | 0.7285× | 0.72× | ✅ |
| upload | 416,543 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,954 | 41,092 | 0.6316× | 0.64× | ✅ |
| client | 18,642 | 19,730 | 0.9449× | 0.93× | ✅ |
| socket | 19,878 | 21,046 | 0.9445× | 0.93× | ✅ |
| pgsql | 22,830 | 24,662 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 86,138 | 90,418 | 0.9527× | 0.95× | ✅ |
| dns | 229,662 | 233,114 | 0.9852× | 0.99× | ✅ |
| file | 109,101 | 131,207 | 0.8315× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
