# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5503605` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,764 | 15,213 | 0.9047× | 0.91× | ✅ |
| echo-1k | 16,735 | 18,165 | 0.9213× | 0.93× | ✅ |
| json | 22,056 | 23,161 | 0.9523× | 0.95× | ✅ |
| service-health | 16,328 | 17,777 | 0.9185× | 0.92× | ✅ |
| websocket | 6,673 | 8,188 | 0.8150× | 0.8× | ✅ |
| timer | 202,938 | 313,919 | 0.6465× | 0.65× | ✅ |
| app | 126,271 | 135,968 | 0.9287× | 0.93× | ✅ |
| http2 | 19,083 | 20,495 | 0.9311× | 0.92× | ✅ |
| table | 193,872 | 201,876 | 0.9603× | 0.96× | ✅ |
| channel | 137,656 | 155,717 | 0.8840× | 0.88× | ✅ |
| task | 32,012 | 33,265 | 0.9623× | 0.96× | ✅ |
| mysql | 49,840 | 51,064 | 0.9760× | 0.97× | ✅ |
| http-client | 33,554 | 35,800 | 0.9373× | 0.93× | ✅ |
| gzip | 194,210 | 197,093 | 0.9854× | 0.99× | ✅ |
| websocket-deflate | 179,076 | 181,569 | 0.9863× | 0.99× | ✅ |
| tcp-length | 4,640 | 4,762 | 0.9743× | 0.96× | ✅ |
| tcp-eof | 5,138 | 10,228 | 0.5023× | 0.5× | ✅ |
| udp | 7,592 | 7,987 | 0.9506× | 0.95× | ✅ |
| connect | 17,442 | 18,686 | 0.9334× | 0.93× | ✅ |
| https | 105,954 | 108,809 | 0.9738× | 0.98× | ✅ |
| wss | 101,128 | 104,794 | 0.9650× | 0.97× | ✅ |
| atomic | 49,811 | 51,276 | 0.9714× | 0.97× | ✅ |
| process-mode | 15,252 | 16,962 | 0.8992× | 0.89× | ✅ |
| http2-concurrent | 367,071 | 409,069 | 0.8973× | 0.9× | ✅ |
| websocket-64k | 89,953 | 159,693 | 0.5633× | 0.57× | ✅ |
| websocket-fragmented | 11,806 | 16,368 | 0.7213× | 0.72× | ✅ |
| upload | 415,990 | 1,322,410 | 0.3146× | 0.32× | ✅ |
| static | 26,638 | 40,960 | 0.6503× | 0.65× | ✅ |
| client | 18,439 | 19,757 | 0.9333× | 0.94× | ✅ |
| socket | 19,595 | 21,069 | 0.9301× | 0.94× | ✅ |
| pgsql | 22,728 | 24,594 | 0.9241× | 0.93× | ✅ |
| sqlite | 23,444 | 25,236 | 0.9290× | 0.93× | ✅ |
| curl | 86,460 | 90,478 | 0.9556× | 0.95× | ✅ |
| dns | 230,592 | 232,615 | 0.9913× | 0.99× | ✅ |

Geometric mean 0.8532×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
