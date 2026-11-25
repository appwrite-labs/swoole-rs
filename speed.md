# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ab5d2a5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,766 | 15,214 | 0.9048× | 0.91× | ✅ |
| echo-1k | 16,798 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,090 | 23,173 | 0.9533× | 0.95× | ✅ |
| service-health | 16,348 | 17,786 | 0.9191× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 202,000 | 313,810 | 0.6437× | 0.65× | ✅ |
| app | 127,525 | 135,898 | 0.9384× | 0.93× | ✅ |
| http2 | 19,041 | 20,496 | 0.9290× | 0.92× | ✅ |
| table | 194,320 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,372 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,090 | 33,269 | 0.9646× | 0.96× | ✅ |
| mysql | 50,292 | 51,085 | 0.9845× | 0.97× | ✅ |
| http-client | 33,598 | 35,796 | 0.9386× | 0.93× | ✅ |
| gzip | 194,294 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,164 | 181,570 | 0.9868× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9538× | 0.95× | ✅ |
| connect | 17,540 | 18,734 | 0.9363× | 0.93× | ✅ |
| https | 106,299 | 108,802 | 0.9770× | 0.98× | ✅ |
| wss | 101,581 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,216 | 51,276 | 0.9793× | 0.97× | ✅ |
| process-mode | 15,435 | 17,024 | 0.9067× | 0.89× | ✅ |
| http2-concurrent | 364,506 | 408,861 | 0.8915× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,508 | 1,322,428 | 0.3150× | 0.32× | ✅ |
| static | 25,914 | 41,063 | 0.6311× | 0.64× | ✅ |
| client | 18,625 | 19,721 | 0.9444× | 0.93× | ✅ |
| socket | 19,866 | 21,036 | 0.9444× | 0.93× | ✅ |
| pgsql | 22,822 | 24,663 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,134 | 90,418 | 0.9526× | 0.95× | ✅ |
| dns | 229,902 | 232,895 | 0.9872× | 0.99× | ✅ |
| file | 108,940 | 131,297 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
