# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ab5d2a5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,769 | 15,214 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,166 | 0.9249× | 0.92× | ✅ |
| json | 22,093 | 23,170 | 0.9535× | 0.95× | ✅ |
| service-health | 16,351 | 17,782 | 0.9195× | 0.92× | ✅ |
| websocket | 6,721 | 8,189 | 0.8208× | 0.81× | ✅ |
| timer | 202,684 | 315,186 | 0.6431× | 0.65× | ✅ |
| app | 127,518 | 135,915 | 0.9382× | 0.93× | ✅ |
| http2 | 19,035 | 20,497 | 0.9287× | 0.92× | ✅ |
| table | 194,320 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,376 | 155,723 | 0.8822× | 0.88× | ✅ |
| task | 32,087 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,295 | 51,085 | 0.9845× | 0.97× | ✅ |
| http-client | 33,600 | 35,796 | 0.9387× | 0.93× | ✅ |
| gzip | 194,296 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,537 | 18,734 | 0.9361× | 0.93× | ✅ |
| https | 106,299 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,213 | 51,276 | 0.9793× | 0.97× | ✅ |
| process-mode | 15,432 | 17,028 | 0.9063× | 0.89× | ✅ |
| http2-concurrent | 364,614 | 408,865 | 0.8918× | 0.89× | ✅ |
| websocket-64k | 90,001 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,511 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,943 | 41,092 | 0.6313× | 0.64× | ✅ |
| client | 18,626 | 19,721 | 0.9445× | 0.93× | ✅ |
| socket | 19,867 | 21,036 | 0.9444× | 0.93× | ✅ |
| pgsql | 22,823 | 24,662 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,134 | 90,306 | 0.9538× | 0.95× | ✅ |
| dns | 229,790 | 233,300 | 0.9850× | 0.99× | ✅ |
| file | 108,940 | 131,296 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
