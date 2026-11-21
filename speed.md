# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `83c55af` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,806 | 18,165 | 0.9251× | 0.92× | ✅ |
| json | 22,097 | 23,173 | 0.9535× | 0.95× | ✅ |
| service-health | 16,354 | 17,786 | 0.9195× | 0.92× | ✅ |
| websocket | 6,721 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 202,014 | 313,750 | 0.6439× | 0.65× | ✅ |
| app | 127,618 | 135,937 | 0.9388× | 0.93× | ✅ |
| http2 | 18,959 | 20,500 | 0.9248× | 0.92× | ✅ |
| table | 194,319 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,375 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,088 | 33,265 | 0.9646× | 0.96× | ✅ |
| mysql | 50,325 | 51,085 | 0.9851× | 0.97× | ✅ |
| http-client | 33,638 | 35,797 | 0.9397× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,323 | 108,809 | 0.9771× | 0.98× | ✅ |
| wss | 101,595 | 104,806 | 0.9694× | 0.97× | ✅ |
| atomic | 50,209 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,438 | 17,021 | 0.9069× | 0.89× | ✅ |
| http2-concurrent | 364,489 | 409,058 | 0.8910× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,369 | 0.7284× | 0.72× | ✅ |
| upload | 416,532 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,954 | 41,063 | 0.6320× | 0.64× | ✅ |
| client | 18,634 | 19,730 | 0.9445× | 0.93× | ✅ |
| socket | 19,873 | 21,045 | 0.9443× | 0.93× | ✅ |
| pgsql | 22,830 | 24,663 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,528 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,146 | 90,417 | 0.9528× | 0.95× | ✅ |
| dns | 230,481 | 232,580 | 0.9910× | 0.99× | ✅ |
| file | 109,101 | 131,296 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8539×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
