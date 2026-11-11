# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0e5ae1d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,260 | 15,214 | 0.9373× | 0.97× | ✅ |
| echo-1k | 17,381 | 18,177 | 0.9562× | 0.99× | ✅ |
| json | 22,342 | 23,170 | 0.9642× | 0.98× | ✅ |
| service-health | 16,849 | 17,786 | 0.9474× | 0.97× | ✅ |
| websocket | 6,855 | 8,191 | 0.8369× | 0.84× | ✅ |
| timer | 202,681 | 313,227 | 0.6471× | 0.65× | ✅ |
| app | 126,536 | 135,936 | 0.9308× | 0.94× | ✅ |
| http2 | 20,159 | 20,464 | 0.9851× | 0.99× | ✅ |
| table | 196,197 | 201,874 | 0.9719× | 0.98× | ✅ |
| channel | 139,553 | 154,667 | 0.9023× | 0.91× | ✅ |
| task | 32,217 | 33,270 | 0.9684× | 0.99× | ✅ |
| mysql | 49,881 | 51,064 | 0.9768× | 0.99× | ✅ |
| http-client | 34,069 | 35,826 | 0.9509× | 0.97× | ✅ |
| client | 18,765 | 19,766 | 0.9493× | 0.97× | ✅ |
| socket | 20,227 | 21,071 | 0.9600× | 0.98× | ✅ |
| gzip | 196,248 | 197,096 | 0.9957× | 1.0× | ✅ |
| websocket-deflate | 181,636 | 181,572 | 1.0004× | 1.0× | ❌ |
| tcp-length | 5,366 | 4,765 | 1.1261× | 1.13× | ➖ |
| tcp-eof | 5,860 | 10,231 | 0.5728× | 0.58× | ✅ |
| udp | 7,982 | 7,987 | 0.9994× | 1.0× | ✅ |
| connect | 19,092 | 18,703 | 1.0208× | 1.05× | ➖ |
| https | 109,484 | 108,803 | 1.0063× | 1.01× | ➖ |
| wss | 105,104 | 104,802 | 1.0029× | 1.0× | ❌ |
| atomic | 50,069 | 51,325 | 0.9755× | 0.99× | ✅ |
| process-mode | 16,525 | 16,959 | 0.9744× | 1.0× | ✅ |
| http2-concurrent | 374,754 | 409,069 | 0.9161× | 0.92× | ✅ |
| websocket-64k | 156,304 | 159,703 | 0.9787× | 0.98× | ✅ |
| websocket-fragmented | 14,216 | 16,202 | 0.8774× | 0.87× | ✅ |
| upload | 673,897 | 1,322,477 | 0.5096× | 0.51× | ✅ |
| static | 35,136 | 41,045 | 0.8560× | 0.86× | ✅ |

Geometric mean 0.9120×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
