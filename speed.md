# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2e4dc43` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,763 | 15,210 | 0.9049× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,084 | 23,173 | 0.9530× | 0.95× | ✅ |
| service-health | 16,342 | 17,786 | 0.9188× | 0.92× | ✅ |
| websocket | 6,723 | 8,192 | 0.8207× | 0.81× | ✅ |
| timer | 202,029 | 313,839 | 0.6437× | 0.65× | ✅ |
| app | 127,626 | 135,942 | 0.9388× | 0.93× | ✅ |
| http2 | 18,963 | 20,500 | 0.9250× | 0.92× | ✅ |
| table | 194,314 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,370 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,089 | 33,268 | 0.9646× | 0.96× | ✅ |
| mysql | 50,288 | 51,085 | 0.9844× | 0.97× | ✅ |
| http-client | 33,669 | 35,793 | 0.9407× | 0.93× | ✅ |
| gzip | 194,291 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,192 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,764 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,069 | 10,231 | 0.4954× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9538× | 0.95× | ✅ |
| connect | 17,538 | 18,734 | 0.9362× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,596 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,209 | 51,272 | 0.9793× | 0.97× | ✅ |
| process-mode | 15,430 | 17,024 | 0.9064× | 0.89× | ✅ |
| http2-concurrent | 364,095 | 408,861 | 0.8905× | 0.89× | ✅ |
| websocket-64k | 90,005 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,925 | 16,368 | 0.7286× | 0.72× | ✅ |
| upload | 416,514 | 1,322,430 | 0.3150× | 0.32× | ✅ |
| static | 25,945 | 41,063 | 0.6318× | 0.64× | ✅ |
| client | 18,634 | 19,721 | 0.9449× | 0.93× | ✅ |
| socket | 19,874 | 21,033 | 0.9449× | 0.93× | ✅ |
| pgsql | 22,830 | 24,662 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,520 | 25,264 | 0.9309× | 0.93× | ✅ |
| curl | 86,142 | 90,402 | 0.9529× | 0.95× | ✅ |
| dns | 229,880 | 232,442 | 0.9890× | 0.99× | ✅ |
| file | 109,183 | 131,300 | 0.8315× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
