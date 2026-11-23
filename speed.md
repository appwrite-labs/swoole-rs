# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `856f62c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,214 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,083 | 23,170 | 0.9531× | 0.95× | ✅ |
| service-health | 16,341 | 17,786 | 0.9188× | 0.92× | ✅ |
| websocket | 6,725 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,990 | 313,795 | 0.6437× | 0.65× | ✅ |
| app | 127,639 | 135,904 | 0.9392× | 0.93× | ✅ |
| http2 | 18,964 | 20,497 | 0.9252× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,366 | 155,723 | 0.8821× | 0.88× | ✅ |
| task | 32,083 | 33,267 | 0.9644× | 0.96× | ✅ |
| mysql | 50,312 | 51,085 | 0.9849× | 0.97× | ✅ |
| http-client | 33,629 | 35,792 | 0.9396× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,533 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,806 | 0.9771× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,416 | 17,021 | 0.9057× | 0.89× | ✅ |
| http2-concurrent | 364,217 | 409,053 | 0.8904× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,528 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,941 | 41,092 | 0.6313× | 0.64× | ✅ |
| client | 18,621 | 19,731 | 0.9438× | 0.93× | ✅ |
| socket | 19,857 | 21,046 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,826 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,132 | 90,547 | 0.9512× | 0.95× | ✅ |
| dns | 229,968 | 232,892 | 0.9874× | 0.99× | ✅ |
| file | 109,092 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
