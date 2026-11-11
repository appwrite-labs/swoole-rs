# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c2e820a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,041 | 15,213 | 0.9230× | 0.93× | ✅ |
| echo-1k | 17,159 | 18,165 | 0.9446× | 0.95× | ✅ |
| json | 22,148 | 23,161 | 0.9563× | 0.96× | ✅ |
| service-health | 16,638 | 17,774 | 0.9361× | 0.94× | ✅ |
| websocket | 6,703 | 8,184 | 0.8190× | 0.82× | ✅ |
| timer | 201,796 | 313,742 | 0.6432× | 0.65× | ✅ |
| app | 125,700 | 135,938 | 0.9247× | 0.93× | ✅ |
| http2 | 19,807 | 20,467 | 0.9678× | 0.98× | ✅ |
| table | 195,848 | 201,865 | 0.9702× | 0.97× | ✅ |
| channel | 138,275 | 154,654 | 0.8941× | 0.9× | ✅ |
| task | 32,037 | 33,265 | 0.9631× | 0.97× | ✅ |
| mysql | 49,558 | 51,062 | 0.9706× | 0.98× | ✅ |
| http-client | 33,560 | 35,809 | 0.9372× | 0.95× | ✅ |
| client | 18,531 | 19,757 | 0.9380× | 0.95× | ✅ |
| socket | 19,501 | 21,067 | 0.9257× | 0.94× | ✅ |
| gzip | 196,024 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,459 | 181,568 | 0.9994× | 1.0× | ✅ |
| tcp-length | 5,196 | 4,762 | 1.0912× | 1.09× | ❌ |
| tcp-eof | 5,731 | 10,231 | 0.5602× | 0.56× | ✅ |
| udp | 7,935 | 7,987 | 0.9934× | 1.0× | ✅ |
| connect | 18,204 | 18,707 | 0.9731× | 0.99× | ✅ |
| https | 108,912 | 108,810 | 1.0009× | 1.01× | ➖ |
| wss | 104,619 | 104,800 | 0.9983× | 1.0× | ✅ |
| atomic | 49,802 | 51,312 | 0.9706× | 0.98× | ✅ |
| process-mode | 16,456 | 16,959 | 0.9704× | 0.98× | ✅ |
| http2-concurrent | 373,894 | 408,882 | 0.9144× | 0.92× | ✅ |
| websocket-64k | 156,157 | 159,696 | 0.9778× | 0.98× | ✅ |
| websocket-fragmented | 12,967 | 16,327 | 0.7942× | 0.79× | ✅ |
| upload | 544,256 | 1,322,381 | 0.4116× | 0.42× | ✅ |
| static | 34,177 | 41,039 | 0.8328× | 0.84× | ✅ |
| pgsql | 23,292 | 24,594 | 0.9471× | 0.96× | ✅ |
| sqlite | 23,579 | 25,232 | 0.9345× | 0.95× | ✅ |
| curl | 85,282 | 90,507 | 0.9423× | 0.95× | ✅ |
| dns | 231,364 | 232,984 | 0.9931× | 1.0× | ✅ |

Geometric mean 0.8993×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
