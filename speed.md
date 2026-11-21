# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2d059ae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,758 | 15,210 | 0.9045× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,083 | 23,170 | 0.9531× | 0.95× | ✅ |
| service-health | 16,341 | 17,786 | 0.9188× | 0.92× | ✅ |
| websocket | 6,725 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 202,473 | 314,912 | 0.6430× | 0.65× | ✅ |
| app | 127,618 | 136,020 | 0.9382× | 0.93× | ✅ |
| http2 | 18,970 | 20,500 | 0.9254× | 0.92× | ✅ |
| table | 194,306 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,366 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,268 | 0.9645× | 0.96× | ✅ |
| mysql | 50,308 | 51,085 | 0.9848× | 0.97× | ✅ |
| http-client | 33,629 | 35,797 | 0.9395× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,597 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,416 | 17,024 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 364,161 | 409,053 | 0.8903× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,510 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,860 | 40,999 | 0.6307× | 0.64× | ✅ |
| client | 18,621 | 19,730 | 0.9438× | 0.93× | ✅ |
| socket | 19,860 | 21,046 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,826 | 24,662 | 0.9256× | 0.92× | ✅ |
| sqlite | 23,527 | 25,264 | 0.9312× | 0.93× | ✅ |
| curl | 86,130 | 90,419 | 0.9526× | 0.95× | ✅ |
| dns | 229,908 | 233,084 | 0.9864× | 0.99× | ✅ |
| file | 109,096 | 131,296 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
