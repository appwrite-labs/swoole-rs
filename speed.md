# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e166301` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,767 | 15,214 | 0.9049× | 0.91× | ✅ |
| echo-1k | 16,799 | 18,165 | 0.9248× | 0.92× | ✅ |
| json | 22,101 | 23,164 | 0.9541× | 0.95× | ✅ |
| service-health | 16,359 | 17,777 | 0.9202× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,972 | 313,869 | 0.6435× | 0.65× | ✅ |
| app | 127,533 | 135,902 | 0.9384× | 0.93× | ✅ |
| http2 | 19,006 | 20,496 | 0.9273× | 0.92× | ✅ |
| table | 194,395 | 202,900 | 0.9581× | 0.96× | ✅ |
| channel | 137,383 | 155,714 | 0.8823× | 0.88× | ✅ |
| task | 32,091 | 33,265 | 0.9647× | 0.96× | ✅ |
| mysql | 50,280 | 51,085 | 0.9842× | 0.97× | ✅ |
| http-client | 33,615 | 35,787 | 0.9393× | 0.93× | ✅ |
| gzip | 194,289 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,194 | 181,565 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,538 | 18,734 | 0.9362× | 0.93× | ✅ |
| https | 106,300 | 108,802 | 0.9770× | 0.98× | ✅ |
| wss | 101,579 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,204 | 51,285 | 0.9789× | 0.97× | ✅ |
| process-mode | 15,432 | 17,021 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 363,931 | 409,057 | 0.8897× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,517 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,868 | 41,005 | 0.6308× | 0.64× | ✅ |
| client | 18,614 | 19,730 | 0.9434× | 0.93× | ✅ |
| socket | 19,857 | 21,045 | 0.9436× | 0.93× | ✅ |
| pgsql | 22,822 | 24,663 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,516 | 25,268 | 0.9307× | 0.93× | ✅ |
| curl | 86,133 | 90,538 | 0.9513× | 0.95× | ✅ |
| dns | 229,806 | 232,721 | 0.9875× | 0.99× | ✅ |
| file | 108,941 | 131,296 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
