# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `95afc7f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,921 | 15,214 | 0.9151× | 0.92× | ✅ |
| echo-1k | 17,019 | 18,165 | 0.9369× | 0.94× | ✅ |
| json | 22,032 | 23,161 | 0.9513× | 0.95× | ✅ |
| service-health | 16,541 | 17,777 | 0.9305× | 0.93× | ✅ |
| websocket | 6,577 | 8,184 | 0.8037× | 0.82× | ✅ |
| timer | 203,631 | 313,600 | 0.6493× | 0.65× | ✅ |
| app | 125,764 | 135,909 | 0.9254× | 0.93× | ✅ |
| http2 | 18,646 | 20,468 | 0.9110× | 0.93× | ✅ |
| table | 195,491 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 138,458 | 154,654 | 0.8953× | 0.9× | ✅ |
| task | 32,100 | 33,291 | 0.9642× | 0.96× | ✅ |
| mysql | 49,589 | 51,055 | 0.9713× | 0.97× | ✅ |
| http-client | 33,767 | 35,809 | 0.9430× | 0.94× | ✅ |
| client | 18,500 | 19,757 | 0.9364× | 0.94× | ✅ |
| socket | 19,634 | 21,067 | 0.9320× | 0.92× | ✅ |
| gzip | 194,130 | 197,079 | 0.9850× | 1.0× | ✅ |
| websocket-deflate | 179,421 | 181,572 | 0.9882× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,762 | 1.0062× | 1.05× | ➖ |
| tcp-eof | 5,340 | 10,231 | 0.5219× | 0.54× | ✅ |
| udp | 7,573 | 7,986 | 0.9483× | 0.95× | ✅ |
| connect | 18,324 | 18,707 | 0.9796× | 0.98× | ✅ |
| https | 108,785 | 108,803 | 0.9998× | 1.0× | ✅ |
| wss | 104,477 | 104,801 | 0.9969× | 1.0× | ✅ |
| atomic | 49,698 | 51,316 | 0.9685× | 0.97× | ✅ |
| process-mode | 16,681 | 16,959 | 0.9836× | 0.99× | ✅ |
| http2-concurrent | 358,645 | 408,880 | 0.8771× | 0.88× | ✅ |
| websocket-64k | 156,023 | 159,693 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 13,024 | 16,327 | 0.7977× | 0.79× | ✅ |
| upload | 477,973 | 1,322,382 | 0.3614× | 0.37× | ✅ |
| static | 29,282 | 41,045 | 0.7134× | 0.72× | ✅ |
| pgsql | 23,201 | 24,594 | 0.9434× | 0.95× | ✅ |
| sqlite | 23,485 | 25,221 | 0.9311× | 0.94× | ✅ |
| curl | 84,748 | 90,300 | 0.9385× | 0.94× | ✅ |
| dns | 231,276 | 232,707 | 0.9938× | 0.99× | ✅ |

Geometric mean 0.8830×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
