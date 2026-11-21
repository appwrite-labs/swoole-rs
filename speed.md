# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `70426a5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,813 | 18,165 | 0.9255× | 0.92× | ✅ |
| json | 22,104 | 23,173 | 0.9539× | 0.95× | ✅ |
| service-health | 16,362 | 17,786 | 0.9199× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,817 | 313,827 | 0.6431× | 0.65× | ✅ |
| app | 127,620 | 135,989 | 0.9385× | 0.93× | ✅ |
| http2 | 18,961 | 20,497 | 0.9251× | 0.92× | ✅ |
| table | 194,327 | 201,885 | 0.9626× | 0.96× | ✅ |
| channel | 137,387 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,095 | 33,268 | 0.9647× | 0.96× | ✅ |
| mysql | 50,326 | 51,085 | 0.9852× | 0.97× | ✅ |
| http-client | 33,650 | 35,787 | 0.9403× | 0.93× | ✅ |
| gzip | 194,280 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,562 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9524× | 0.95× | ✅ |
| connect | 17,537 | 18,734 | 0.9361× | 0.93× | ✅ |
| https | 106,331 | 108,802 | 0.9773× | 0.98× | ✅ |
| wss | 101,589 | 104,800 | 0.9694× | 0.97× | ✅ |
| atomic | 50,217 | 51,285 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,426 | 17,021 | 0.9063× | 0.89× | ✅ |
| http2-concurrent | 364,265 | 409,056 | 0.8905× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,372 | 0.7285× | 0.72× | ✅ |
| upload | 416,521 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,925 | 41,063 | 0.6313× | 0.64× | ✅ |
| client | 18,642 | 19,726 | 0.9450× | 0.93× | ✅ |
| socket | 19,881 | 21,045 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,832 | 24,662 | 0.9258× | 0.92× | ✅ |
| sqlite | 23,532 | 25,268 | 0.9313× | 0.93× | ✅ |
| curl | 86,140 | 90,419 | 0.9527× | 0.95× | ✅ |
| dns | 230,019 | 232,781 | 0.9881× | 0.99× | ✅ |
| file | 109,101 | 131,296 | 0.8310× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
