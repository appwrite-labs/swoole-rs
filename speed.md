# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7635436` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,214 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,084 | 23,164 | 0.9533× | 0.95× | ✅ |
| service-health | 16,338 | 17,777 | 0.9190× | 0.92× | ✅ |
| websocket | 6,726 | 8,186 | 0.8216× | 0.81× | ✅ |
| timer | 202,143 | 313,781 | 0.6442× | 0.65× | ✅ |
| app | 127,621 | 135,956 | 0.9387× | 0.93× | ✅ |
| http2 | 18,956 | 20,496 | 0.9249× | 0.92× | ✅ |
| table | 194,374 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,366 | 155,717 | 0.8821× | 0.88× | ✅ |
| task | 32,083 | 33,269 | 0.9644× | 0.96× | ✅ |
| mysql | 50,305 | 51,085 | 0.9847× | 0.97× | ✅ |
| http-client | 33,660 | 35,796 | 0.9403× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,533 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,193 | 51,285 | 0.9787× | 0.97× | ✅ |
| process-mode | 15,412 | 17,025 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,122 | 409,056 | 0.8902× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,369 | 0.7287× | 0.72× | ✅ |
| upload | 416,524 | 1,322,430 | 0.3150× | 0.32× | ✅ |
| static | 25,941 | 41,088 | 0.6313× | 0.64× | ✅ |
| client | 18,618 | 19,729 | 0.9436× | 0.93× | ✅ |
| socket | 19,860 | 21,045 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,826 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,129 | 90,423 | 0.9525× | 0.95× | ✅ |
| dns | 230,131 | 233,072 | 0.9874× | 0.99× | ✅ |
| file | 109,096 | 131,297 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
