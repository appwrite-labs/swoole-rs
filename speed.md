# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ee79f02` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,768 | 15,214 | 0.9050× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,097 | 23,173 | 0.9535× | 0.95× | ✅ |
| service-health | 16,354 | 17,786 | 0.9195× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,994 | 313,676 | 0.6440× | 0.65× | ✅ |
| app | 127,588 | 135,962 | 0.9384× | 0.93× | ✅ |
| http2 | 18,962 | 20,497 | 0.9251× | 0.92× | ✅ |
| table | 194,323 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,891 | 155,726 | 0.8855× | 0.88× | ✅ |
| task | 32,088 | 33,269 | 0.9645× | 0.96× | ✅ |
| mysql | 50,321 | 51,085 | 0.9850× | 0.97× | ✅ |
| http-client | 33,636 | 35,796 | 0.9397× | 0.93× | ✅ |
| gzip | 194,280 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,153 | 181,573 | 0.9867× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,331 | 108,810 | 0.9772× | 0.98× | ✅ |
| wss | 101,599 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,206 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,402 | 17,024 | 0.9047× | 0.89× | ✅ |
| http2-concurrent | 364,268 | 409,056 | 0.8905× | 0.89× | ✅ |
| websocket-64k | 90,001 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,368 | 0.7284× | 0.72× | ✅ |
| upload | 416,413 | 1,322,432 | 0.3149× | 0.32× | ✅ |
| static | 25,890 | 41,028 | 0.6310× | 0.65× | ✅ |
| client | 18,634 | 19,729 | 0.9445× | 0.93× | ✅ |
| socket | 19,873 | 21,045 | 0.9443× | 0.93× | ✅ |
| pgsql | 22,831 | 24,663 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,492 | 25,268 | 0.9297× | 0.93× | ✅ |
| curl | 85,479 | 90,512 | 0.9444× | 0.95× | ✅ |
| dns | 229,492 | 232,597 | 0.9866× | 0.99× | ✅ |
| file | 109,092 | 131,296 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
