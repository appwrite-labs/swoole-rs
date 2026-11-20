# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2fd1126` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,768 | 15,214 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,814 | 18,162 | 0.9258× | 0.92× | ✅ |
| json | 22,105 | 23,170 | 0.9540× | 0.95× | ✅ |
| service-health | 16,362 | 17,786 | 0.9199× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,988 | 313,772 | 0.6437× | 0.65× | ✅ |
| app | 127,660 | 135,876 | 0.9395× | 0.93× | ✅ |
| http2 | 18,965 | 20,496 | 0.9253× | 0.92× | ✅ |
| table | 194,331 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,383 | 155,723 | 0.8822× | 0.88× | ✅ |
| task | 32,084 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,678 | 35,793 | 0.9409× | 0.93× | ✅ |
| gzip | 194,292 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,537 | 18,734 | 0.9361× | 0.93× | ✅ |
| https | 106,331 | 108,806 | 0.9773× | 0.98× | ✅ |
| wss | 101,599 | 104,803 | 0.9694× | 0.97× | ✅ |
| atomic | 50,217 | 51,281 | 0.9793× | 0.97× | ✅ |
| process-mode | 15,435 | 17,024 | 0.9067× | 0.89× | ✅ |
| http2-concurrent | 363,370 | 409,057 | 0.8883× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,521 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,873 | 40,999 | 0.6311× | 0.64× | ✅ |
| client | 18,643 | 19,730 | 0.9449× | 0.93× | ✅ |
| socket | 19,878 | 21,046 | 0.9445× | 0.93× | ✅ |
| pgsql | 22,833 | 24,663 | 0.9258× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 86,134 | 90,549 | 0.9512× | 0.95× | ✅ |
| dns | 229,946 | 232,973 | 0.9870× | 0.99× | ✅ |
| file | 109,092 | 130,918 | 0.8333× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
