# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `be7677e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,759 | 15,210 | 0.9046× | 0.91× | ✅ |
| echo-1k | 16,801 | 18,162 | 0.9251× | 0.92× | ✅ |
| json | 22,083 | 23,165 | 0.9533× | 0.95× | ✅ |
| service-health | 16,338 | 17,777 | 0.9190× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,965 | 314,004 | 0.6432× | 0.65× | ✅ |
| app | 127,603 | 135,968 | 0.9385× | 0.93× | ✅ |
| http2 | 18,969 | 20,500 | 0.9253× | 0.92× | ✅ |
| table | 194,374 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,362 | 155,717 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,265 | 0.9646× | 0.96× | ✅ |
| mysql | 50,305 | 51,085 | 0.9847× | 0.97× | ✅ |
| http-client | 33,661 | 35,792 | 0.9404× | 0.93× | ✅ |
| gzip | 194,291 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9785× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,595 | 104,800 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,276 | 0.9789× | 0.97× | ✅ |
| process-mode | 15,413 | 17,021 | 0.9055× | 0.89× | ✅ |
| http2-concurrent | 364,641 | 408,864 | 0.8918× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,695 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,528 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,912 | 41,063 | 0.6310× | 0.64× | ✅ |
| client | 18,621 | 19,721 | 0.9442× | 0.93× | ✅ |
| socket | 19,857 | 21,036 | 0.9439× | 0.93× | ✅ |
| pgsql | 22,826 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,132 | 90,417 | 0.9526× | 0.95× | ✅ |
| dns | 230,168 | 232,839 | 0.9885× | 0.99× | ✅ |
| file | 109,096 | 131,296 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
