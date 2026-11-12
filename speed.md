# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3921232` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,928 | 15,210 | 0.9157× | 0.92× | ✅ |
| echo-1k | 17,040 | 18,165 | 0.9381× | 0.94× | ✅ |
| json | 22,011 | 23,161 | 0.9503× | 0.96× | ✅ |
| service-health | 16,507 | 17,773 | 0.9287× | 0.93× | ✅ |
| websocket | 6,715 | 8,191 | 0.8197× | 0.82× | ✅ |
| timer | 202,486 | 313,551 | 0.6458× | 0.65× | ✅ |
| app | 125,895 | 135,929 | 0.9262× | 0.93× | ✅ |
| http2 | 19,824 | 20,464 | 0.9687× | 0.97× | ✅ |
| table | 195,720 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,867 | 154,654 | 0.8979× | 0.9× | ✅ |
| task | 31,937 | 33,264 | 0.9601× | 0.96× | ✅ |
| mysql | 49,568 | 51,046 | 0.9710× | 0.97× | ✅ |
| http-client | 33,608 | 35,802 | 0.9387× | 0.94× | ✅ |
| client | 18,464 | 19,756 | 0.9346× | 0.94× | ✅ |
| socket | 19,743 | 21,067 | 0.9372× | 0.92× | ✅ |
| gzip | 196,007 | 197,083 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,477 | 181,572 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,023 | 4,765 | 1.0540× | 1.05× | ❌ |
| tcp-eof | 5,565 | 10,231 | 0.5439× | 0.54× | ✅ |
| udp | 8,038 | 7,986 | 1.0065× | 1.0× | ❌ |
| connect | 18,251 | 18,703 | 0.9758× | 0.98× | ✅ |
| https | 108,775 | 108,813 | 0.9996× | 1.01× | ✅ |
| wss | 104,591 | 104,800 | 0.9980× | 1.0× | ✅ |
| atomic | 49,671 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,436 | 16,963 | 0.9689× | 0.97× | ✅ |
| http2-concurrent | 372,331 | 408,880 | 0.9106× | 0.91× | ✅ |
| websocket-64k | 156,164 | 159,692 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,084 | 16,327 | 0.8014× | 0.79× | ✅ |
| upload | 544,682 | 1,322,384 | 0.4119× | 0.42× | ✅ |
| static | 34,434 | 41,045 | 0.8389× | 0.84× | ✅ |
| pgsql | 23,213 | 24,594 | 0.9439× | 0.95× | ✅ |
| sqlite | 23,493 | 25,217 | 0.9316× | 0.94× | ✅ |
| curl | 85,328 | 90,494 | 0.9429× | 0.95× | ✅ |
| dns | 231,042 | 232,993 | 0.9916× | 1.0× | ✅ |

Geometric mean 0.8977×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
