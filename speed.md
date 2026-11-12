# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `8355ad4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,928 | 15,210 | 0.9157× | 0.92× | ✅ |
| echo-1k | 17,041 | 18,162 | 0.9383× | 0.94× | ✅ |
| json | 22,020 | 23,158 | 0.9509× | 0.96× | ✅ |
| service-health | 16,516 | 17,777 | 0.9291× | 0.93× | ✅ |
| websocket | 6,719 | 8,188 | 0.8206× | 0.82× | ✅ |
| timer | 202,493 | 313,802 | 0.6453× | 0.65× | ✅ |
| app | 125,985 | 135,976 | 0.9265× | 0.93× | ✅ |
| http2 | 19,826 | 20,464 | 0.9688× | 0.97× | ✅ |
| table | 195,729 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,873 | 154,655 | 0.8980× | 0.9× | ✅ |
| task | 31,928 | 33,264 | 0.9599× | 0.96× | ✅ |
| mysql | 49,570 | 51,059 | 0.9709× | 0.97× | ✅ |
| http-client | 33,581 | 35,809 | 0.9378× | 0.94× | ✅ |
| client | 18,469 | 19,756 | 0.9348× | 0.94× | ✅ |
| socket | 19,752 | 21,067 | 0.9376× | 0.92× | ✅ |
| gzip | 196,003 | 197,079 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,474 | 181,568 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,023 | 4,765 | 1.0540× | 1.05× | ❌ |
| tcp-eof | 5,565 | 10,231 | 0.5439× | 0.54× | ✅ |
| udp | 8,037 | 7,987 | 1.0064× | 1.0× | ❌ |
| connect | 18,251 | 18,707 | 0.9757× | 0.98× | ✅ |
| https | 108,825 | 108,810 | 1.0001× | 1.01× | ➖ |
| wss | 104,669 | 104,794 | 0.9988× | 1.0× | ✅ |
| atomic | 49,680 | 51,316 | 0.9681× | 0.97× | ✅ |
| process-mode | 16,436 | 16,956 | 0.9693× | 0.97× | ✅ |
| http2-concurrent | 372,154 | 409,342 | 0.9092× | 0.91× | ✅ |
| websocket-64k | 156,163 | 159,698 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,076 | 16,331 | 0.8007× | 0.79× | ✅ |
| upload | 544,677 | 1,322,382 | 0.4119× | 0.42× | ✅ |
| static | 34,449 | 41,045 | 0.8393× | 0.84× | ✅ |
| pgsql | 23,212 | 24,594 | 0.9438× | 0.95× | ✅ |
| sqlite | 23,493 | 25,232 | 0.9311× | 0.94× | ✅ |
| curl | 85,329 | 90,374 | 0.9442× | 0.95× | ✅ |
| dns | 230,919 | 233,135 | 0.9905× | 1.0× | ✅ |

Geometric mean 0.8977×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
