# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `6b7b537` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,758 | 15,213 | 0.9043× | 0.91× | ✅ |
| echo-1k | 16,808 | 18,165 | 0.9253× | 0.92× | ✅ |
| json | 22,094 | 23,173 | 0.9534× | 0.95× | ✅ |
| service-health | 16,349 | 17,786 | 0.9192× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,943 | 313,818 | 0.6435× | 0.65× | ✅ |
| app | 127,658 | 135,845 | 0.9397× | 0.93× | ✅ |
| http2 | 18,984 | 20,493 | 0.9264× | 0.92× | ✅ |
| table | 194,321 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,373 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,268 | 0.9645× | 0.96× | ✅ |
| mysql | 50,315 | 51,045 | 0.9857× | 0.97× | ✅ |
| http-client | 33,635 | 35,793 | 0.9397× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,530 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,299 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,577 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,207 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,421 | 17,021 | 0.9060× | 0.89× | ✅ |
| http2-concurrent | 363,557 | 409,053 | 0.8888× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,692 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,519 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,907 | 41,063 | 0.6309× | 0.64× | ✅ |
| client | 18,632 | 19,730 | 0.9443× | 0.93× | ✅ |
| socket | 19,871 | 21,046 | 0.9442× | 0.93× | ✅ |
| pgsql | 22,822 | 24,662 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,519 | 25,253 | 0.9313× | 0.93× | ✅ |
| curl | 86,128 | 90,423 | 0.9525× | 0.95× | ✅ |
| dns | 230,335 | 232,754 | 0.9896× | 0.99× | ✅ |
| file | 109,070 | 131,300 | 0.8307× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
