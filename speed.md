# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f1831f8` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,213 | 0.9044× | 0.91× | ✅ |
| echo-1k | 16,803 | 18,162 | 0.9252× | 0.92× | ✅ |
| json | 22,088 | 23,170 | 0.9533× | 0.95× | ✅ |
| service-health | 16,345 | 17,782 | 0.9192× | 0.92× | ✅ |
| websocket | 6,726 | 8,192 | 0.8210× | 0.81× | ✅ |
| timer | 201,998 | 314,003 | 0.6433× | 0.65× | ✅ |
| app | 127,581 | 135,951 | 0.9384× | 0.93× | ✅ |
| http2 | 18,995 | 20,497 | 0.9267× | 0.92× | ✅ |
| table | 194,314 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,366 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,083 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,318 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,680 | 35,787 | 0.9411× | 0.93× | ✅ |
| gzip | 194,277 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,562 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,531 | 18,734 | 0.9358× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,592 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,200 | 51,285 | 0.9789× | 0.97× | ✅ |
| process-mode | 15,427 | 17,024 | 0.9062× | 0.89× | ✅ |
| http2-concurrent | 363,979 | 409,053 | 0.8898× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,692 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,521 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,935 | 41,063 | 0.6316× | 0.64× | ✅ |
| client | 18,625 | 19,727 | 0.9442× | 0.93× | ✅ |
| socket | 19,864 | 21,045 | 0.9439× | 0.93× | ✅ |
| pgsql | 22,821 | 24,663 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,520 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,132 | 90,419 | 0.9526× | 0.95× | ✅ |
| dns | 229,722 | 232,899 | 0.9864× | 0.99× | ✅ |
| file | 108,955 | 131,300 | 0.8298× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
