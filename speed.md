# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c6d1bf2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,732 | 15,214 | 0.9026× | 0.91× | ✅ |
| echo-1k | 16,724 | 18,165 | 0.9207× | 0.92× | ✅ |
| json | 22,061 | 23,173 | 0.9520× | 0.95× | ✅ |
| service-health | 16,305 | 17,786 | 0.9167× | 0.92× | ✅ |
| websocket | 6,676 | 8,189 | 0.8153× | 0.8× | ✅ |
| timer | 201,287 | 313,983 | 0.6411× | 0.65× | ✅ |
| app | 126,770 | 135,905 | 0.9328× | 0.93× | ✅ |
| http2 | 18,967 | 20,496 | 0.9254× | 0.92× | ✅ |
| table | 193,897 | 201,885 | 0.9604× | 0.96× | ✅ |
| channel | 138,479 | 155,723 | 0.8893× | 0.88× | ✅ |
| task | 31,935 | 33,265 | 0.9600× | 0.96× | ✅ |
| mysql | 49,833 | 51,061 | 0.9760× | 0.97× | ✅ |
| http-client | 33,656 | 35,793 | 0.9403× | 0.93× | ✅ |
| gzip | 194,346 | 197,095 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,766 | 0.9744× | 0.96× | ✅ |
| tcp-eof | 5,194 | 10,231 | 0.5077× | 0.5× | ✅ |
| udp | 7,584 | 7,986 | 0.9496× | 0.95× | ✅ |
| connect | 17,569 | 18,734 | 0.9378× | 0.93× | ✅ |
| https | 106,024 | 108,802 | 0.9745× | 0.98× | ✅ |
| wss | 101,239 | 104,797 | 0.9661× | 0.97× | ✅ |
| atomic | 50,164 | 51,276 | 0.9783× | 0.97× | ✅ |
| process-mode | 15,255 | 17,021 | 0.8962× | 0.89× | ✅ |
| http2-concurrent | 364,152 | 408,861 | 0.8906× | 0.89× | ✅ |
| websocket-64k | 89,975 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,840 | 16,368 | 0.7234× | 0.72× | ✅ |
| upload | 416,224 | 1,322,429 | 0.3147× | 0.32× | ✅ |
| static | 27,094 | 41,063 | 0.6598× | 0.65× | ✅ |
| client | 18,381 | 19,720 | 0.9321× | 0.93× | ✅ |
| socket | 19,632 | 21,036 | 0.9332× | 0.93× | ✅ |
| pgsql | 22,587 | 24,618 | 0.9175× | 0.92× | ✅ |
| sqlite | 23,401 | 25,236 | 0.9273× | 0.93× | ✅ |
| curl | 85,347 | 90,359 | 0.9445× | 0.95× | ✅ |
| dns | 229,536 | 232,685 | 0.9865× | 0.99× | ✅ |

Geometric mean 0.8532×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
