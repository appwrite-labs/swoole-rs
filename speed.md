# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `1cb11b3` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,765 | 15,210 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,094 | 23,170 | 0.9536× | 0.95× | ✅ |
| service-health | 16,351 | 17,786 | 0.9193× | 0.92× | ✅ |
| websocket | 6,726 | 8,185 | 0.8217× | 0.81× | ✅ |
| timer | 202,032 | 313,861 | 0.6437× | 0.65× | ✅ |
| app | 127,532 | 135,965 | 0.9380× | 0.93× | ✅ |
| http2 | 19,040 | 20,493 | 0.9291× | 0.92× | ✅ |
| table | 194,320 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,372 | 155,723 | 0.8822× | 0.88× | ✅ |
| task | 32,087 | 33,268 | 0.9645× | 0.96× | ✅ |
| mysql | 50,296 | 51,082 | 0.9846× | 0.97× | ✅ |
| http-client | 33,598 | 35,795 | 0.9386× | 0.93× | ✅ |
| gzip | 194,300 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,194 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,618 | 7,986 | 0.9539× | 0.95× | ✅ |
| connect | 17,540 | 18,734 | 0.9363× | 0.93× | ✅ |
| https | 106,299 | 108,802 | 0.9770× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,216 | 51,276 | 0.9793× | 0.97× | ✅ |
| process-mode | 15,432 | 17,021 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 364,506 | 408,861 | 0.8915× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,692 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,511 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,850 | 40,999 | 0.6305× | 0.64× | ✅ |
| client | 18,625 | 19,721 | 0.9445× | 0.93× | ✅ |
| socket | 19,866 | 21,033 | 0.9445× | 0.93× | ✅ |
| pgsql | 22,822 | 24,663 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,132 | 90,420 | 0.9526× | 0.95× | ✅ |
| dns | 230,132 | 232,644 | 0.9892× | 0.99× | ✅ |
| file | 108,940 | 131,300 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
