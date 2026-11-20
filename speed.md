# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `97ce799` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,165 | 0.9256× | 0.92× | ✅ |
| json | 22,101 | 23,173 | 0.9537× | 0.95× | ✅ |
| service-health | 16,359 | 17,786 | 0.9197× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 202,150 | 313,684 | 0.6444× | 0.65× | ✅ |
| app | 127,634 | 135,908 | 0.9391× | 0.93× | ✅ |
| http2 | 18,960 | 20,497 | 0.9250× | 0.92× | ✅ |
| table | 194,331 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,387 | 155,723 | 0.8823× | 0.88× | ✅ |
| task | 32,092 | 33,269 | 0.9646× | 0.96× | ✅ |
| mysql | 50,318 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,650 | 35,797 | 0.9400× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,331 | 108,802 | 0.9773× | 0.98× | ✅ |
| wss | 101,595 | 104,800 | 0.9694× | 0.97× | ✅ |
| atomic | 50,217 | 51,285 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,435 | 17,024 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 363,350 | 409,057 | 0.8883× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,529 | 1,322,427 | 0.3150× | 0.32× | ✅ |
| static | 25,937 | 41,063 | 0.6316× | 0.64× | ✅ |
| client | 18,642 | 19,730 | 0.9449× | 0.93× | ✅ |
| socket | 19,878 | 21,045 | 0.9445× | 0.93× | ✅ |
| pgsql | 22,828 | 24,663 | 0.9256× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 86,139 | 90,416 | 0.9527× | 0.95× | ✅ |
| dns | 229,943 | 232,590 | 0.9886× | 0.99× | ✅ |
| file | 109,092 | 131,297 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
