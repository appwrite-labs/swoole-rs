# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3be396f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,923 | 15,213 | 0.9152× | 0.92× | ✅ |
| echo-1k | 17,071 | 18,165 | 0.9398× | 0.94× | ✅ |
| json | 22,005 | 23,161 | 0.9501× | 0.96× | ✅ |
| service-health | 16,502 | 17,777 | 0.9283× | 0.93× | ✅ |
| websocket | 6,714 | 8,188 | 0.8200× | 0.82× | ✅ |
| timer | 202,484 | 313,564 | 0.6458× | 0.65× | ✅ |
| app | 125,935 | 135,861 | 0.9269× | 0.93× | ✅ |
| http2 | 19,827 | 20,468 | 0.9687× | 0.97× | ✅ |
| table | 195,719 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,803 | 154,654 | 0.8975× | 0.9× | ✅ |
| task | 31,967 | 33,268 | 0.9609× | 0.96× | ✅ |
| mysql | 49,563 | 51,064 | 0.9706× | 0.97× | ✅ |
| http-client | 33,578 | 35,806 | 0.9378× | 0.94× | ✅ |
| client | 18,462 | 19,757 | 0.9344× | 0.94× | ✅ |
| socket | 19,742 | 21,064 | 0.9372× | 0.92× | ✅ |
| gzip | 196,006 | 197,083 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,476 | 181,572 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,022 | 4,765 | 1.0538× | 1.05× | ❌ |
| tcp-eof | 5,564 | 10,231 | 0.5438× | 0.54× | ✅ |
| udp | 8,033 | 7,986 | 1.0059× | 1.0× | ❌ |
| connect | 18,254 | 18,707 | 0.9758× | 0.98× | ✅ |
| https | 108,817 | 108,810 | 1.0001× | 1.01× | ➖ |
| wss | 104,668 | 104,800 | 0.9987× | 1.0× | ✅ |
| atomic | 49,670 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,422 | 16,959 | 0.9683× | 0.97× | ✅ |
| http2-concurrent | 372,673 | 409,550 | 0.9100× | 0.91× | ✅ |
| websocket-64k | 156,164 | 159,693 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,071 | 16,327 | 0.8006× | 0.79× | ✅ |
| upload | 544,652 | 1,322,380 | 0.4119× | 0.42× | ✅ |
| static | 34,446 | 41,045 | 0.8392× | 0.84× | ✅ |
| pgsql | 23,218 | 24,595 | 0.9440× | 0.95× | ✅ |
| sqlite | 23,487 | 25,232 | 0.9309× | 0.94× | ✅ |
| curl | 85,236 | 90,385 | 0.9430× | 0.95× | ✅ |
| dns | 231,146 | 233,022 | 0.9919× | 1.0× | ✅ |

Geometric mean 0.8977×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
