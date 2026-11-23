# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7856d3d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,084 | 23,170 | 0.9531× | 0.95× | ✅ |
| service-health | 16,341 | 17,786 | 0.9188× | 0.92× | ✅ |
| websocket | 6,726 | 8,192 | 0.8210× | 0.81× | ✅ |
| timer | 202,003 | 313,781 | 0.6438× | 0.65× | ✅ |
| app | 127,600 | 135,948 | 0.9386× | 0.93× | ✅ |
| http2 | 18,963 | 20,496 | 0.9252× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,366 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,265 | 0.9646× | 0.96× | ✅ |
| mysql | 50,309 | 51,085 | 0.9848× | 0.97× | ✅ |
| http-client | 33,664 | 35,788 | 0.9407× | 0.93× | ✅ |
| gzip | 194,279 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,565 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,730 | 0.9360× | 0.93× | ✅ |
| https | 106,313 | 108,809 | 0.9771× | 0.98× | ✅ |
| wss | 101,597 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,413 | 17,024 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,323 | 409,056 | 0.8906× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,692 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,510 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,860 | 40,999 | 0.6307× | 0.64× | ✅ |
| client | 18,621 | 19,730 | 0.9438× | 0.93× | ✅ |
| socket | 19,860 | 21,046 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,818 | 24,662 | 0.9252× | 0.92× | ✅ |
| sqlite | 23,512 | 25,268 | 0.9305× | 0.93× | ✅ |
| curl | 86,121 | 90,418 | 0.9525× | 0.95× | ✅ |
| dns | 230,258 | 233,085 | 0.9879× | 0.99× | ✅ |
| file | 109,106 | 131,297 | 0.8310× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
