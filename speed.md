# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `7245c22` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,363 | 15,214 | 0.9441× | 0.91× | ✅ |
| echo-1k | 17,363 | 18,167 | 0.9557× | 0.92× | ✅ |
| json | 22,369 | 23,175 | 0.9652× | 0.94× | ✅ |
| service-health | 16,936 | 17,795 | 0.9517× | 0.93× | ✅ |
| websocket | 6,560 | 8,188 | 0.8012× | 0.81× | ✅ |
| timer | 201,403 | 313,350 | 0.6427× | 0.67× | ✅ |
| app | 125,543 | 135,923 | 0.9236× | 0.92× | ✅ |
| http2 | 19,789 | 20,461 | 0.9672× | 0.95× | ✅ |
| table | 196,873 | 201,883 | 0.9752× | 0.98× | ✅ |
| channel | 139,758 | 154,674 | 0.9036× | 0.89× | ✅ |
| task | 31,884 | 33,266 | 0.9585× | 0.94× | ✅ |
| mysql | 49,410 | 51,085 | 0.9672× | 0.97× | ✅ |
| http-client | 33,565 | 35,837 | 0.9366× | 0.95× | ✅ |
| client | 18,442 | 19,177 | 0.9617× | 0.95× | ✅ |
| socket | 19,776 | 20,074 | 0.9852× | 0.99× | ✅ |
| gzip | 200,686 | 197,097 | 1.0182× | 1.02× | ➖ |
| websocket-deflate | 181,368 | 181,580 | 0.9988× | 1.0× | ✅ |
| tcp-length | 5,054 | 4,746 | 1.0649× | 1.02× | ❌ |
| tcp-eof | 5,592 | 10,212 | 0.5476× | 0.55× | ✅ |
| udp | 8,014 | 7,326 | 1.0938× | 1.09× | ❌ |
| connect | 20,312 | 18,716 | 1.0853× | 1.04× | ❌ |
| https | 109,160 | 108,810 | 1.0032× | 1.04× | ➖ |
| wss | 104,259 | 104,794 | 0.9949× | 1.04× | ✅ |
| atomic | 52,329 | 51,334 | 1.0194× | 1.02× | ➖ |
| process-mode | 16,529 | 17,014 | 0.9715× | 0.95× | ✅ |
| http2-concurrent | 1,459,077 | 409,615 | 3.5621× | 1.31× | ❌ |
| websocket-64k | 156,010 | 159,690 | 0.9770× | 0.62× | ✅ |
| websocket-fragmented | 13,549 | 16,194 | 0.8367× | 0.78× | ✅ |
| upload | 1,780,592 | 1,322,439 | 1.3464× | 1.05× | ❌ |
| static | 39,242 | 40,990 | 0.9573× | 1.0× | ✅ |

Geometric mean 0.9892×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
