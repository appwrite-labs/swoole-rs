# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d2a0089` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,770 | 15,214 | 0.9051× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,166 | 0.9249× | 0.92× | ✅ |
| json | 22,090 | 23,173 | 0.9532× | 0.95× | ✅ |
| service-health | 16,351 | 17,786 | 0.9193× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,778 | 313,780 | 0.6431× | 0.65× | ✅ |
| app | 127,521 | 135,890 | 0.9384× | 0.93× | ✅ |
| http2 | 19,032 | 20,496 | 0.9286× | 0.92× | ✅ |
| table | 194,320 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,376 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,087 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,295 | 51,088 | 0.9845× | 0.97× | ✅ |
| http-client | 33,597 | 35,795 | 0.9386× | 0.93× | ✅ |
| gzip | 194,296 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9537× | 0.95× | ✅ |
| connect | 17,541 | 18,734 | 0.9363× | 0.93× | ✅ |
| https | 106,299 | 108,802 | 0.9770× | 0.98× | ✅ |
| wss | 101,573 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,213 | 51,276 | 0.9793× | 0.97× | ✅ |
| process-mode | 15,431 | 17,025 | 0.9064× | 0.89× | ✅ |
| http2-concurrent | 364,633 | 408,865 | 0.8918× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,511 | 1,322,427 | 0.3150× | 0.32× | ✅ |
| static | 25,879 | 41,028 | 0.6308× | 0.64× | ✅ |
| client | 18,623 | 19,720 | 0.9443× | 0.93× | ✅ |
| socket | 19,863 | 21,037 | 0.9442× | 0.93× | ✅ |
| pgsql | 22,820 | 24,663 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,515 | 25,268 | 0.9306× | 0.93× | ✅ |
| curl | 86,143 | 90,365 | 0.9533× | 0.95× | ✅ |
| dns | 229,801 | 232,443 | 0.9886× | 0.99× | ✅ |
| file | 108,940 | 130,919 | 0.8321× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
