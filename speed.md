# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f6bf569` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,210 | 0.9043× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,084 | 23,170 | 0.9531× | 0.95× | ✅ |
| service-health | 16,341 | 17,782 | 0.9190× | 0.92× | ✅ |
| websocket | 6,726 | 8,192 | 0.8210× | 0.81× | ✅ |
| timer | 201,994 | 313,773 | 0.6438× | 0.65× | ✅ |
| app | 127,630 | 136,004 | 0.9384× | 0.93× | ✅ |
| http2 | 18,964 | 20,496 | 0.9253× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,366 | 155,723 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,308 | 51,077 | 0.9849× | 0.97× | ✅ |
| http-client | 33,666 | 35,785 | 0.9408× | 0.93× | ✅ |
| gzip | 194,275 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,565 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,599 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,193 | 51,285 | 0.9787× | 0.97× | ✅ |
| process-mode | 15,416 | 17,021 | 0.9057× | 0.89× | ✅ |
| http2-concurrent | 364,225 | 409,053 | 0.8904× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,369 | 0.7287× | 0.72× | ✅ |
| upload | 416,527 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,912 | 41,063 | 0.6310× | 0.64× | ✅ |
| client | 18,621 | 19,731 | 0.9438× | 0.93× | ✅ |
| socket | 19,857 | 21,046 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,826 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,131 | 90,548 | 0.9512× | 0.95× | ✅ |
| dns | 230,281 | 232,665 | 0.9898× | 0.99× | ✅ |
| file | 109,092 | 130,922 | 0.8333× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
