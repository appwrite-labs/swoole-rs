# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4edd1c3` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 24,913 | 15,213 | 1.6376× | 0.95× | ❌ |
| echo-1k | 32,666 | 18,168 | 1.7981× | 0.96× | ❌ |
| json | 33,484 | 23,178 | 1.4447× | 0.97× | ❌ |
| service-health | 27,705 | 17,791 | 1.5572× | 0.96× | ❌ |
| websocket | 6,574 | 8,191 | 0.8025× | 0.81× | ✅ |
| timer | 211,873 | 313,356 | 0.6761× | 0.65× | ✅ |
| app | 140,807 | 135,951 | 1.0357× | 0.92× | ❌ |
| http2 | 19,608 | 20,464 | 0.9582× | 0.97× | ✅ |
| table | 208,115 | 202,907 | 1.0257× | 0.98× | ❌ |
| channel | 148,291 | 155,189 | 0.9556× | 0.89× | ✅ |
| task | 46,970 | 33,265 | 1.4120× | 0.96× | ❌ |
| mysql | 60,097 | 51,079 | 1.1765× | 0.97× | ❌ |
| http-client | 52,565 | 35,821 | 1.4674× | 0.94× | ❌ |
| client | 29,376 | 19,775 | 1.4855× | 0.95× | ❌ |
| socket | 30,856 | 21,090 | 1.4630× | 0.99× | ❌ |
| gzip | 215,843 | 197,090 | 1.0951× | 1.02× | ❌ |
| websocket-deflate | 181,372 | 181,560 | 0.9990× | 1.0× | ✅ |
| tcp-length | 5,044 | 4,743 | 1.0635× | 1.07× | ➖ |
| tcp-eof | 5,582 | 10,209 | 0.5468× | 0.55× | ✅ |
| udp | 7,558 | 7,326 | 1.0316× | 1.07× | ➖ |
| connect | 33,484 | 18,703 | 1.7903× | 1.09× | ❌ |
| https | 119,691 | 108,810 | 1.1000× | 1.01× | ❌ |
| wss | 104,272 | 104,801 | 0.9950× | 1.0× | ✅ |
| atomic | 66,311 | 51,334 | 1.2918× | 1.02× | ❌ |
| process-mode | 27,039 | 16,949 | 1.5954× | 0.98× | ❌ |
| http2-concurrent | 1,456,318 | 408,813 | 3.5623× | 0.93× | ❌ |
| websocket-64k | 156,021 | 159,691 | 0.9770× | 0.62× | ✅ |
| websocket-fragmented | 13,917 | 16,194 | 0.8594× | 0.78× | ✅ |
| upload | 1,799,895 | 1,322,472 | 1.3610× | 1.05× | ❌ |
| static | 39,233 | 40,999 | 0.9569× | 0.96× | ✅ |

Geometric mean 1.1914×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
