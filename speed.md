# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4f19b47` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,214 | 0.9044× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,166 | 0.9247× | 0.92× | ✅ |
| json | 22,080 | 23,173 | 0.9528× | 0.95× | ✅ |
| service-health | 16,338 | 17,786 | 0.9186× | 0.92× | ✅ |
| websocket | 6,722 | 8,185 | 0.8212× | 0.81× | ✅ |
| timer | 202,105 | 313,734 | 0.6442× | 0.65× | ✅ |
| app | 127,638 | 135,903 | 0.9392× | 0.93× | ✅ |
| http2 | 18,964 | 20,496 | 0.9252× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,366 | 155,723 | 0.8821× | 0.88× | ✅ |
| task | 32,083 | 33,264 | 0.9645× | 0.96× | ✅ |
| mysql | 50,312 | 51,088 | 0.9848× | 0.97× | ✅ |
| http-client | 33,657 | 35,793 | 0.9403× | 0.93× | ✅ |
| gzip | 194,287 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,608 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,730 | 0.9360× | 0.93× | ✅ |
| https | 106,313 | 108,813 | 0.9770× | 0.98× | ✅ |
| wss | 101,595 | 104,800 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,412 | 17,021 | 0.9054× | 0.89× | ✅ |
| http2-concurrent | 364,353 | 409,057 | 0.8907× | 0.89× | ✅ |
| websocket-64k | 90,005 | 159,695 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7282× | 0.72× | ✅ |
| upload | 416,524 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,848 | 40,999 | 0.6304× | 0.64× | ✅ |
| client | 18,623 | 19,730 | 0.9439× | 0.93× | ✅ |
| socket | 19,860 | 21,045 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,826 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,125 | 90,533 | 0.9513× | 0.95× | ✅ |
| dns | 230,199 | 232,733 | 0.9891× | 0.99× | ✅ |
| file | 109,096 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
