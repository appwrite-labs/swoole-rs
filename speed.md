# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `abcf70e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,161 | 15,210 | 0.9310× | 0.93× | ✅ |
| echo-1k | 17,286 | 18,162 | 0.9518× | 0.95× | ✅ |
| json | 22,246 | 23,170 | 0.9601× | 0.96× | ✅ |
| service-health | 16,741 | 17,786 | 0.9412× | 0.94× | ✅ |
| websocket | 6,724 | 8,188 | 0.8212× | 0.83× | ✅ |
| timer | 202,364 | 314,856 | 0.6427× | 0.65× | ✅ |
| app | 126,594 | 135,856 | 0.9318× | 0.93× | ✅ |
| http2 | 19,942 | 20,468 | 0.9743× | 0.98× | ✅ |
| table | 195,815 | 201,874 | 0.9700× | 0.97× | ✅ |
| channel | 139,441 | 154,664 | 0.9016× | 0.91× | ✅ |
| task | 32,064 | 33,264 | 0.9639× | 0.97× | ✅ |
| mysql | 49,922 | 50,985 | 0.9792× | 0.98× | ✅ |
| http-client | 34,097 | 35,824 | 0.9518× | 0.95× | ✅ |
| client | 18,780 | 19,766 | 0.9501× | 0.94× | ✅ |
| socket | 19,739 | 21,071 | 0.9368× | 0.94× | ✅ |
| gzip | 196,138 | 197,079 | 0.9952× | 1.0× | ✅ |
| websocket-deflate | 181,485 | 181,568 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,229 | 4,765 | 1.0972× | 1.1× | ➖ |
| tcp-eof | 5,802 | 10,228 | 0.5672× | 0.57× | ✅ |
| udp | 7,989 | 7,987 | 1.0003× | 1.0× | ❌ |
| connect | 18,382 | 18,707 | 0.9826× | 0.99× | ✅ |
| https | 109,254 | 108,813 | 1.0040× | 1.01× | ➖ |
| wss | 104,833 | 104,802 | 1.0003× | 1.0× | ❌ |
| atomic | 49,958 | 51,321 | 0.9734× | 0.98× | ✅ |
| process-mode | 16,595 | 16,956 | 0.9787× | 0.98× | ✅ |
| http2-concurrent | 373,623 | 409,074 | 0.9133× | 0.92× | ✅ |
| websocket-64k | 156,171 | 159,705 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 12,973 | 16,335 | 0.7942× | 0.79× | ✅ |
| upload | 544,464 | 1,322,385 | 0.4117× | 0.42× | ✅ |
| static | 34,534 | 40,981 | 0.8427× | 0.85× | ✅ |
| pgsql | 23,457 | 24,595 | 0.9537× | 0.96× | ✅ |
| sqlite | 23,774 | 25,232 | 0.9422× | 0.95× | ✅ |
| curl | 85,647 | 90,494 | 0.9464× | 0.95× | ✅ |
| dns | 231,275 | 232,944 | 0.9928× | 1.0× | ✅ |

Geometric mean 0.9041×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
