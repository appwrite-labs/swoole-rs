# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0eb1622` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,767 | 15,210 | 0.9051× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,165 | 0.9256× | 0.92× | ✅ |
| json | 22,104 | 23,174 | 0.9539× | 0.95× | ✅ |
| service-health | 16,362 | 17,786 | 0.9199× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,970 | 313,741 | 0.6437× | 0.65× | ✅ |
| app | 127,655 | 135,974 | 0.9388× | 0.93× | ✅ |
| http2 | 18,959 | 20,496 | 0.9250× | 0.92× | ✅ |
| table | 194,331 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,387 | 155,727 | 0.8822× | 0.88× | ✅ |
| task | 32,085 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,678 | 35,793 | 0.9409× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,534 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,331 | 108,806 | 0.9773× | 0.98× | ✅ |
| wss | 101,595 | 104,800 | 0.9694× | 0.97× | ✅ |
| atomic | 50,217 | 51,285 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,435 | 17,024 | 0.9067× | 0.89× | ✅ |
| http2-concurrent | 363,718 | 409,058 | 0.8892× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7282× | 0.72× | ✅ |
| upload | 416,535 | 1,322,430 | 0.3150× | 0.32× | ✅ |
| static | 25,867 | 41,005 | 0.6308× | 0.64× | ✅ |
| client | 18,642 | 19,726 | 0.9451× | 0.93× | ✅ |
| socket | 19,882 | 21,046 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,830 | 24,663 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 86,136 | 90,422 | 0.9526× | 0.95× | ✅ |
| dns | 230,348 | 233,139 | 0.9880× | 0.99× | ✅ |
| file | 109,101 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
