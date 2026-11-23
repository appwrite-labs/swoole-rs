# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `856f62c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,213 | 0.9044× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,080 | 23,173 | 0.9528× | 0.95× | ✅ |
| service-health | 16,338 | 17,786 | 0.9186× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,963 | 313,802 | 0.6436× | 0.65× | ✅ |
| app | 127,637 | 135,953 | 0.9388× | 0.93× | ✅ |
| http2 | 18,967 | 20,500 | 0.9252× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,083 | 33,269 | 0.9644× | 0.96× | ✅ |
| mysql | 50,308 | 51,086 | 0.9848× | 0.97× | ✅ |
| http-client | 33,628 | 35,792 | 0.9396× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,813 | 0.9770× | 0.98× | ✅ |
| wss | 101,591 | 104,800 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,276 | 0.9789× | 0.97× | ✅ |
| process-mode | 15,412 | 17,021 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 364,489 | 408,861 | 0.8915× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,524 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,912 | 41,063 | 0.6310× | 0.64× | ✅ |
| client | 18,621 | 19,721 | 0.9442× | 0.93× | ✅ |
| socket | 19,860 | 21,036 | 0.9441× | 0.93× | ✅ |
| pgsql | 22,818 | 24,663 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,519 | 25,253 | 0.9313× | 0.93× | ✅ |
| curl | 86,133 | 90,419 | 0.9526× | 0.95× | ✅ |
| dns | 230,366 | 232,864 | 0.9893× | 0.99× | ✅ |
| file | 109,088 | 131,293 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
