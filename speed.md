# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `cabb2cb` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,762 | 15,210 | 0.9048× | 0.91× | ✅ |
| echo-1k | 16,798 | 18,166 | 0.9247× | 0.92× | ✅ |
| json | 22,083 | 23,173 | 0.9529× | 0.95× | ✅ |
| service-health | 16,341 | 17,786 | 0.9187× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,984 | 313,764 | 0.6437× | 0.65× | ✅ |
| app | 127,599 | 135,903 | 0.9389× | 0.93× | ✅ |
| http2 | 19,043 | 20,500 | 0.9289× | 0.92× | ✅ |
| table | 194,313 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,365 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,085 | 33,265 | 0.9645× | 0.96× | ✅ |
| mysql | 50,335 | 51,088 | 0.9853× | 0.97× | ✅ |
| http-client | 33,643 | 35,796 | 0.9399× | 0.93× | ✅ |
| gzip | 194,300 | 197,096 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9538× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,299 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,582 | 104,800 | 0.9693× | 0.97× | ✅ |
| atomic | 50,199 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,432 | 17,025 | 0.9064× | 0.89× | ✅ |
| http2-concurrent | 364,510 | 409,057 | 0.8911× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,696 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,520 | 1,322,430 | 0.3150× | 0.32× | ✅ |
| static | 25,862 | 40,999 | 0.6308× | 0.64× | ✅ |
| client | 18,632 | 19,730 | 0.9443× | 0.93× | ✅ |
| socket | 19,871 | 21,045 | 0.9442× | 0.93× | ✅ |
| pgsql | 22,819 | 24,663 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,133 | 90,417 | 0.9526× | 0.95× | ✅ |
| dns | 230,092 | 232,734 | 0.9886× | 0.99× | ✅ |
| file | 108,940 | 131,300 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
