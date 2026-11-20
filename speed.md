# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `aa1495e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,768 | 15,214 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,096 | 23,164 | 0.9539× | 0.95× | ✅ |
| service-health | 16,359 | 17,777 | 0.9202× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,975 | 313,790 | 0.6437× | 0.65× | ✅ |
| app | 127,644 | 135,902 | 0.9392× | 0.93× | ✅ |
| http2 | 18,954 | 20,500 | 0.9246× | 0.92× | ✅ |
| table | 194,391 | 202,900 | 0.9581× | 0.96× | ✅ |
| channel | 137,383 | 155,717 | 0.8823× | 0.88× | ✅ |
| task | 32,085 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,321 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,683 | 35,797 | 0.9410× | 0.93× | ✅ |
| gzip | 194,300 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,537 | 18,734 | 0.9361× | 0.93× | ✅ |
| https | 106,323 | 108,806 | 0.9772× | 0.98× | ✅ |
| wss | 101,599 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,218 | 51,285 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,425 | 17,024 | 0.9061× | 0.89× | ✅ |
| http2-concurrent | 363,559 | 409,057 | 0.8888× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,522 | 1,322,430 | 0.3150× | 0.32× | ✅ |
| static | 25,873 | 40,999 | 0.6311× | 0.64× | ✅ |
| client | 18,640 | 19,729 | 0.9448× | 0.93× | ✅ |
| socket | 19,882 | 21,045 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,832 | 24,663 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 86,136 | 90,388 | 0.9530× | 0.95× | ✅ |
| dns | 229,754 | 232,872 | 0.9866× | 0.99× | ✅ |
| file | 109,092 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
