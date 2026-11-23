# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f6bf569` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,083 | 23,165 | 0.9533× | 0.95× | ✅ |
| service-health | 16,338 | 17,777 | 0.9190× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,981 | 313,832 | 0.6436× | 0.65× | ✅ |
| app | 127,637 | 135,961 | 0.9388× | 0.93× | ✅ |
| http2 | 18,960 | 20,497 | 0.9250× | 0.92× | ✅ |
| table | 194,370 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,366 | 155,717 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,265 | 0.9646× | 0.96× | ✅ |
| mysql | 50,308 | 51,085 | 0.9848× | 0.97× | ✅ |
| http-client | 33,628 | 35,796 | 0.9394× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,730 | 0.9360× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,597 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,412 | 17,024 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,428 | 409,053 | 0.8909× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,524 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,912 | 41,063 | 0.6310× | 0.64× | ✅ |
| client | 18,624 | 19,730 | 0.9439× | 0.93× | ✅ |
| socket | 19,860 | 21,045 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,825 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,131 | 90,551 | 0.9512× | 0.95× | ✅ |
| dns | 229,969 | 233,086 | 0.9866× | 0.99× | ✅ |
| file | 109,096 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
