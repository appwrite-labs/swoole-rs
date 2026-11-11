# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `87ed93b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,162 | 15,213 | 0.9309× | 0.94× | ✅ |
| echo-1k | 17,320 | 18,165 | 0.9535× | 0.97× | ✅ |
| json | 22,238 | 23,170 | 0.9598× | 0.97× | ✅ |
| service-health | 16,733 | 17,786 | 0.9408× | 0.95× | ✅ |
| websocket | 6,860 | 8,188 | 0.8378× | 0.84× | ✅ |
| timer | 202,045 | 313,565 | 0.6443× | 0.65× | ✅ |
| app | 126,089 | 135,928 | 0.9276× | 0.94× | ✅ |
| http2 | 20,241 | 20,468 | 0.9889× | 0.99× | ✅ |
| table | 195,879 | 201,874 | 0.9703× | 0.98× | ✅ |
| channel | 139,428 | 154,667 | 0.9015× | 0.91× | ✅ |
| task | 32,013 | 33,268 | 0.9623× | 0.97× | ✅ |
| mysql | 49,646 | 51,060 | 0.9723× | 0.98× | ✅ |
| http-client | 33,790 | 35,830 | 0.9431× | 0.96× | ✅ |
| client | 18,571 | 19,762 | 0.9397× | 0.95× | ✅ |
| socket | 19,971 | 21,071 | 0.9478× | 0.97× | ✅ |
| gzip | 196,173 | 197,079 | 0.9954× | 1.0× | ✅ |
| websocket-deflate | 181,629 | 181,568 | 1.0003× | 1.0× | ❌ |
| tcp-length | 5,373 | 4,765 | 1.1275× | 1.13× | ➖ |
| tcp-eof | 5,929 | 10,231 | 0.5795× | 0.58× | ✅ |
| udp | 8,019 | 7,986 | 1.0041× | 1.0× | ❌ |
| connect | 18,550 | 18,707 | 0.9916× | 0.99× | ✅ |
| https | 109,229 | 108,803 | 1.0039× | 1.01× | ➖ |
| wss | 104,914 | 104,802 | 1.0011× | 1.0× | ❌ |
| atomic | 49,947 | 51,325 | 0.9732× | 0.98× | ✅ |
| process-mode | 16,443 | 16,956 | 0.9697× | 0.98× | ✅ |
| http2-concurrent | 378,897 | 409,074 | 0.9262× | 0.92× | ✅ |
| websocket-64k | 156,307 | 159,705 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 13,172 | 16,339 | 0.8062× | 0.79× | ✅ |
| upload | 675,219 | 1,322,380 | 0.5106× | 0.51× | ✅ |
| static | 34,667 | 41,013 | 0.8453× | 0.85× | ✅ |
| pgsql | 23,409 | 24,594 | 0.9518× | 0.97× | ✅ |
| sqlite | 23,743 | 25,232 | 0.9410× | 0.96× | ✅ |
| curl | 85,799 | 90,361 | 0.9495× | 0.96× | ✅ |
| dns | 231,835 | 232,486 | 0.9972× | 1.0× | ✅ |

Geometric mean 0.9127×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
