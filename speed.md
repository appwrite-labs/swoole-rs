# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9f3a7e1` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,920 | 15,213 | 0.9150× | 0.92× | ✅ |
| echo-1k | 17,021 | 18,165 | 0.9370× | 0.94× | ✅ |
| json | 22,034 | 23,161 | 0.9513× | 0.95× | ✅ |
| service-health | 16,544 | 17,777 | 0.9306× | 0.93× | ✅ |
| websocket | 6,580 | 8,188 | 0.8037× | 0.82× | ✅ |
| timer | 203,444 | 313,514 | 0.6489× | 0.65× | ✅ |
| app | 125,848 | 135,906 | 0.9260× | 0.93× | ✅ |
| http2 | 18,656 | 20,467 | 0.9115× | 0.93× | ✅ |
| table | 195,493 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 138,457 | 154,654 | 0.8953× | 0.9× | ✅ |
| task | 32,090 | 33,268 | 0.9646× | 0.96× | ✅ |
| mysql | 49,534 | 51,064 | 0.9700× | 0.97× | ✅ |
| http-client | 33,766 | 35,808 | 0.9430× | 0.94× | ✅ |
| client | 18,498 | 19,756 | 0.9363× | 0.94× | ✅ |
| socket | 19,639 | 21,067 | 0.9322× | 0.92× | ✅ |
| gzip | 194,120 | 197,079 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,415 | 181,568 | 0.9881× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,765 | 1.0055× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,231 | 0.5219× | 0.54× | ✅ |
| udp | 7,573 | 7,986 | 0.9483× | 0.95× | ✅ |
| connect | 18,333 | 18,707 | 0.9800× | 0.98× | ✅ |
| https | 108,802 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,481 | 104,803 | 0.9969× | 1.0× | ✅ |
| atomic | 49,700 | 51,312 | 0.9686× | 0.97× | ✅ |
| process-mode | 16,679 | 16,959 | 0.9835× | 0.99× | ✅ |
| http2-concurrent | 358,515 | 408,880 | 0.8768× | 0.88× | ✅ |
| websocket-64k | 156,027 | 159,697 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,823 | 16,327 | 0.7854× | 0.79× | ✅ |
| upload | 478,086 | 1,322,380 | 0.3615× | 0.37× | ✅ |
| static | 29,286 | 41,074 | 0.7130× | 0.72× | ✅ |
| pgsql | 23,219 | 24,595 | 0.9441× | 0.95× | ✅ |
| sqlite | 23,487 | 25,221 | 0.9312× | 0.94× | ✅ |
| curl | 84,749 | 90,347 | 0.9380× | 0.94× | ✅ |
| dns | 231,437 | 232,842 | 0.9940× | 0.99× | ✅ |

Geometric mean 0.8826×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
