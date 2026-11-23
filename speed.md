# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b9ac760` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,808 | 18,162 | 0.9254× | 0.92× | ✅ |
| json | 22,101 | 23,161 | 0.9542× | 0.95× | ✅ |
| service-health | 16,350 | 17,773 | 0.9199× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,956 | 313,754 | 0.6437× | 0.65× | ✅ |
| app | 127,635 | 135,971 | 0.9387× | 0.93× | ✅ |
| http2 | 18,960 | 20,496 | 0.9251× | 0.92× | ✅ |
| table | 194,383 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,375 | 155,714 | 0.8822× | 0.88× | ✅ |
| task | 32,083 | 33,268 | 0.9644× | 0.96× | ✅ |
| mysql | 50,321 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,657 | 35,797 | 0.9402× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,524 | 18,734 | 0.9354× | 0.93× | ✅ |
| https | 106,296 | 108,802 | 0.9770× | 0.98× | ✅ |
| wss | 101,571 | 104,800 | 0.9692× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,408 | 17,021 | 0.9052× | 0.89× | ✅ |
| http2-concurrent | 363,787 | 409,057 | 0.8893× | 0.89× | ✅ |
| websocket-64k | 90,001 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,519 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,832 | 40,999 | 0.6301× | 0.64× | ✅ |
| client | 18,624 | 19,730 | 0.9439× | 0.93× | ✅ |
| socket | 19,856 | 21,045 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,819 | 24,663 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,516 | 25,268 | 0.9307× | 0.93× | ✅ |
| curl | 86,126 | 90,546 | 0.9512× | 0.95× | ✅ |
| dns | 229,902 | 232,921 | 0.9870× | 0.99× | ✅ |
| file | 109,092 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
