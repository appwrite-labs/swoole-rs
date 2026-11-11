# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `75fad8a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,405 | 15,213 | 0.9469× | 0.94× | ✅ |
| echo-1k | 17,534 | 18,165 | 0.9653× | 0.97× | ✅ |
| json | 22,510 | 23,170 | 0.9715× | 0.97× | ✅ |
| service-health | 17,008 | 17,786 | 0.9563× | 0.95× | ✅ |
| websocket | 6,849 | 8,191 | 0.8361× | 0.84× | ✅ |
| timer | 202,345 | 313,690 | 0.6450× | 0.65× | ✅ |
| app | 126,138 | 135,966 | 0.9277× | 0.94× | ✅ |
| http2 | 20,185 | 20,463 | 0.9864× | 0.99× | ✅ |
| table | 196,163 | 201,874 | 0.9717× | 0.98× | ✅ |
| channel | 139,712 | 154,664 | 0.9033× | 0.91× | ✅ |
| task | 32,214 | 33,265 | 0.9684× | 0.97× | ✅ |
| mysql | 49,778 | 51,061 | 0.9749× | 0.98× | ✅ |
| http-client | 33,988 | 35,833 | 0.9485× | 0.96× | ✅ |
| client | 18,800 | 19,766 | 0.9511× | 0.95× | ✅ |
| socket | 20,189 | 21,071 | 0.9582× | 0.97× | ✅ |
| gzip | 196,395 | 197,079 | 0.9965× | 1.0× | ✅ |
| websocket-deflate | 181,625 | 181,572 | 1.0003× | 1.0× | ❌ |
| tcp-length | 5,358 | 4,762 | 1.1251× | 1.13× | ➖ |
| tcp-eof | 5,914 | 10,231 | 0.5780× | 0.58× | ✅ |
| udp | 8,020 | 7,987 | 1.0042× | 1.0× | ❌ |
| connect | 18,800 | 18,707 | 1.0050× | 0.99× | ❌ |
| https | 109,539 | 108,803 | 1.0068× | 1.01× | ➖ |
| wss | 104,967 | 104,808 | 1.0015× | 1.0× | ❌ |
| atomic | 50,224 | 51,325 | 0.9786× | 0.98× | ✅ |
| process-mode | 16,710 | 16,959 | 0.9853× | 0.98× | ✅ |
| http2-concurrent | 378,161 | 409,074 | 0.9244× | 0.92× | ✅ |
| websocket-64k | 156,301 | 159,706 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 13,122 | 16,335 | 0.8033× | 0.79× | ✅ |
| upload | 675,459 | 1,322,380 | 0.5108× | 0.51× | ✅ |
| static | 34,664 | 41,016 | 0.8451× | 0.85× | ✅ |
| pgsql | 23,700 | 24,594 | 0.9636× | 0.97× | ✅ |
| sqlite | 24,010 | 25,228 | 0.9517× | 0.96× | ✅ |
| curl | 86,009 | 90,384 | 0.9516× | 0.96× | ✅ |
| dns | 231,291 | 232,670 | 0.9941× | 1.0× | ✅ |

Geometric mean 0.9167×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
