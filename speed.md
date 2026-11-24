# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e166301` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,771 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,803 | 18,162 | 0.9252× | 0.92× | ✅ |
| json | 22,095 | 23,170 | 0.9536× | 0.95× | ✅ |
| service-health | 16,352 | 17,786 | 0.9194× | 0.92× | ✅ |
| websocket | 6,725 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 202,333 | 314,671 | 0.6430× | 0.65× | ✅ |
| app | 127,497 | 135,978 | 0.9376× | 0.93× | ✅ |
| http2 | 18,999 | 20,500 | 0.9268× | 0.92× | ✅ |
| table | 194,321 | 201,889 | 0.9625× | 0.96× | ✅ |
| channel | 137,377 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,091 | 33,268 | 0.9646× | 0.96× | ✅ |
| mysql | 50,293 | 51,085 | 0.9845× | 0.97× | ✅ |
| http-client | 33,600 | 35,791 | 0.9388× | 0.93× | ✅ |
| gzip | 194,297 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,541 | 18,734 | 0.9363× | 0.93× | ✅ |
| https | 106,300 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,217 | 51,272 | 0.9794× | 0.97× | ✅ |
| process-mode | 15,432 | 17,021 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 364,208 | 408,861 | 0.8908× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,368 | 0.7285× | 0.72× | ✅ |
| upload | 416,509 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,850 | 40,999 | 0.6305× | 0.64× | ✅ |
| client | 18,624 | 19,721 | 0.9444× | 0.93× | ✅ |
| socket | 19,867 | 21,036 | 0.9444× | 0.93× | ✅ |
| pgsql | 22,821 | 24,663 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,520 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,131 | 90,422 | 0.9525× | 0.95× | ✅ |
| dns | 230,150 | 232,822 | 0.9885× | 0.99× | ✅ |
| file | 108,941 | 131,300 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
