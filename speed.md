# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a712d64` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,877 | 15,210 | 0.9124× | 0.91× | ✅ |
| echo-1k | 16,918 | 18,162 | 0.9315× | 0.93× | ✅ |
| json | 22,029 | 23,161 | 0.9511× | 0.95× | ✅ |
| service-health | 16,463 | 17,773 | 0.9263× | 0.93× | ✅ |
| websocket | 6,521 | 8,191 | 0.7961× | 0.8× | ✅ |
| timer | 206,637 | 313,740 | 0.6586× | 0.65× | ✅ |
| app | 126,098 | 135,951 | 0.9275× | 0.93× | ✅ |
| http2 | 18,899 | 20,464 | 0.9235× | 0.92× | ✅ |
| table | 194,923 | 201,865 | 0.9656× | 0.97× | ✅ |
| channel | 137,920 | 155,170 | 0.8888× | 0.88× | ✅ |
| task | 32,538 | 33,265 | 0.9782× | 0.96× | ✅ |
| mysql | 49,647 | 51,064 | 0.9722× | 0.97× | ✅ |
| http-client | 33,515 | 35,793 | 0.9363× | 0.93× | ✅ |
| gzip | 193,991 | 197,079 | 0.9843× | 0.99× | ✅ |
| websocket-deflate | 178,595 | 181,560 | 0.9837× | 0.99× | ✅ |
| tcp-length | 4,526 | 4,763 | 0.9502× | 0.96× | ✅ |
| tcp-eof | 5,108 | 10,231 | 0.4992× | 0.5× | ✅ |
| udp | 7,443 | 7,986 | 0.9320× | 0.93× | ✅ |
| connect | 17,516 | 18,707 | 0.9364× | 0.93× | ✅ |
| https | 105,913 | 108,813 | 0.9733× | 0.98× | ✅ |
| wss | 100,763 | 104,797 | 0.9615× | 0.97× | ✅ |
| atomic | 49,970 | 51,316 | 0.9738× | 0.97× | ✅ |
| process-mode | 15,044 | 16,956 | 0.8873× | 0.88× | ✅ |
| http2-concurrent | 361,643 | 408,881 | 0.8845× | 0.88× | ✅ |
| websocket-64k | 89,773 | 159,699 | 0.5621× | 0.57× | ✅ |
| websocket-fragmented | 11,679 | 16,327 | 0.7153× | 0.72× | ✅ |
| upload | 414,010 | 1,322,387 | 0.3131× | 0.32× | ✅ |
| static | 26,744 | 40,981 | 0.6526× | 0.65× | ✅ |
| client | 18,543 | 19,757 | 0.9386× | 0.94× | ✅ |
| socket | 19,702 | 21,072 | 0.9350× | 0.92× | ✅ |
| pgsql | 23,025 | 24,595 | 0.9362× | 0.93× | ✅ |
| sqlite | 23,532 | 25,236 | 0.9325× | 0.93× | ✅ |
| curl | 84,692 | 90,378 | 0.9371× | 0.94× | ✅ |
| dns | 229,465 | 232,681 | 0.9862× | 0.99× | ✅ |

Geometric mean 0.8518×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
