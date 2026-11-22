# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ddc05fe` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,214 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,083 | 23,170 | 0.9531× | 0.95× | ✅ |
| service-health | 16,341 | 17,786 | 0.9188× | 0.92× | ✅ |
| websocket | 6,725 | 8,192 | 0.8209× | 0.81× | ✅ |
| timer | 202,015 | 313,838 | 0.6437× | 0.65× | ✅ |
| app | 127,629 | 135,929 | 0.9389× | 0.93× | ✅ |
| http2 | 18,959 | 20,496 | 0.9250× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,366 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,084 | 33,268 | 0.9644× | 0.96× | ✅ |
| mysql | 50,312 | 51,085 | 0.9849× | 0.97× | ✅ |
| http-client | 33,625 | 35,795 | 0.9394× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,599 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,196 | 51,272 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,415 | 17,024 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 364,448 | 408,861 | 0.8914× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,525 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,912 | 41,063 | 0.6310× | 0.64× | ✅ |
| client | 18,624 | 19,720 | 0.9444× | 0.93× | ✅ |
| socket | 19,860 | 21,036 | 0.9441× | 0.93× | ✅ |
| pgsql | 22,826 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,523 | 25,268 | 0.9309× | 0.93× | ✅ |
| curl | 86,134 | 90,415 | 0.9526× | 0.95× | ✅ |
| dns | 229,709 | 233,114 | 0.9854× | 0.99× | ✅ |
| file | 109,096 | 131,297 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
