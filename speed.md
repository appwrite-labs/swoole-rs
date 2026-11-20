# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ba37b46` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,214 | 0.9052× | 0.91× | ✅ |
| echo-1k | 16,810 | 18,165 | 0.9254× | 0.92× | ✅ |
| json | 22,101 | 23,173 | 0.9537× | 0.95× | ✅ |
| service-health | 16,359 | 17,786 | 0.9197× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,810 | 313,896 | 0.6429× | 0.65× | ✅ |
| app | 127,627 | 135,993 | 0.9385× | 0.93× | ✅ |
| http2 | 18,957 | 20,500 | 0.9247× | 0.92× | ✅ |
| table | 194,331 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,387 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,085 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,678 | 35,792 | 0.9409× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,764 | 0.9784× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9524× | 0.95× | ✅ |
| connect | 17,534 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,331 | 108,802 | 0.9773× | 0.98× | ✅ |
| wss | 101,599 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,217 | 51,281 | 0.9793× | 0.97× | ✅ |
| process-mode | 15,434 | 17,024 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 363,680 | 409,053 | 0.8891× | 0.89× | ✅ |
| websocket-64k | 90,001 | 159,692 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,531 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,867 | 41,005 | 0.6308× | 0.64× | ✅ |
| client | 18,645 | 19,730 | 0.9450× | 0.93× | ✅ |
| socket | 19,881 | 21,046 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,831 | 24,662 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 86,130 | 90,420 | 0.9525× | 0.95× | ✅ |
| dns | 230,213 | 232,549 | 0.9900× | 0.99× | ✅ |
| file | 109,097 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
