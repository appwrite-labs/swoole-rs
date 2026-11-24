# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e166301` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,771 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,803 | 18,165 | 0.9250× | 0.92× | ✅ |
| json | 22,094 | 23,173 | 0.9534× | 0.95× | ✅ |
| service-health | 16,349 | 17,786 | 0.9192× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 202,010 | 313,725 | 0.6439× | 0.65× | ✅ |
| app | 127,523 | 135,954 | 0.9380× | 0.93× | ✅ |
| http2 | 19,012 | 20,500 | 0.9274× | 0.92× | ✅ |
| table | 194,317 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,373 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,091 | 33,268 | 0.9646× | 0.96× | ✅ |
| mysql | 50,297 | 51,085 | 0.9846× | 0.97× | ✅ |
| http-client | 33,600 | 35,795 | 0.9387× | 0.93× | ✅ |
| gzip | 194,297 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,194 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,538 | 18,734 | 0.9362× | 0.93× | ✅ |
| https | 106,300 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,217 | 51,276 | 0.9794× | 0.97× | ✅ |
| process-mode | 15,435 | 17,025 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 364,350 | 408,865 | 0.8911× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,512 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,850 | 40,999 | 0.6305× | 0.64× | ✅ |
| client | 18,624 | 19,721 | 0.9444× | 0.93× | ✅ |
| socket | 19,867 | 21,036 | 0.9444× | 0.93× | ✅ |
| pgsql | 22,824 | 24,663 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,520 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,140 | 90,418 | 0.9527× | 0.95× | ✅ |
| dns | 230,353 | 232,960 | 0.9888× | 0.99× | ✅ |
| file | 108,941 | 131,300 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
