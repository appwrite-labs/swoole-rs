# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2ac8ca0` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,214 | 0.9044× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,083 | 23,173 | 0.9530× | 0.95× | ✅ |
| service-health | 16,341 | 17,786 | 0.9188× | 0.92× | ✅ |
| websocket | 6,725 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,982 | 313,816 | 0.6436× | 0.65× | ✅ |
| app | 127,638 | 135,875 | 0.9394× | 0.93× | ✅ |
| http2 | 18,961 | 20,496 | 0.9251× | 0.92× | ✅ |
| table | 194,310 | 201,889 | 0.9625× | 0.96× | ✅ |
| channel | 137,366 | 155,723 | 0.8821× | 0.88× | ✅ |
| task | 32,088 | 33,265 | 0.9646× | 0.96× | ✅ |
| mysql | 50,309 | 51,088 | 0.9847× | 0.97× | ✅ |
| http-client | 33,633 | 35,781 | 0.9400× | 0.93× | ✅ |
| gzip | 194,275 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,565 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,802 | 0.9771× | 0.98× | ✅ |
| wss | 101,597 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,412 | 17,028 | 0.9051× | 0.89× | ✅ |
| http2-concurrent | 364,169 | 409,053 | 0.8903× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,524 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,848 | 40,999 | 0.6304× | 0.64× | ✅ |
| client | 18,621 | 19,731 | 0.9438× | 0.93× | ✅ |
| socket | 19,861 | 21,046 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,826 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,130 | 90,419 | 0.9526× | 0.95× | ✅ |
| dns | 229,699 | 232,543 | 0.9878× | 0.99× | ✅ |
| file | 109,096 | 131,207 | 0.8315× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
