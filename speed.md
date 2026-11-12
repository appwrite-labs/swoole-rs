# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f3ba82b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,927 | 15,214 | 0.9154× | 0.92× | ✅ |
| echo-1k | 17,043 | 18,165 | 0.9382× | 0.94× | ✅ |
| json | 22,024 | 23,161 | 0.9509× | 0.95× | ✅ |
| service-health | 16,512 | 17,777 | 0.9289× | 0.93× | ✅ |
| websocket | 6,728 | 8,184 | 0.8220× | 0.82× | ✅ |
| timer | 202,198 | 313,805 | 0.6443× | 0.65× | ✅ |
| app | 125,984 | 135,911 | 0.9270× | 0.93× | ✅ |
| http2 | 19,176 | 20,468 | 0.9369× | 0.94× | ✅ |
| table | 195,716 | 201,865 | 0.9695× | 0.97× | ✅ |
| channel | 138,864 | 154,654 | 0.8979× | 0.9× | ✅ |
| task | 31,940 | 33,261 | 0.9603× | 0.96× | ✅ |
| mysql | 49,560 | 51,059 | 0.9706× | 0.97× | ✅ |
| http-client | 33,639 | 35,802 | 0.9396× | 0.94× | ✅ |
| client | 18,467 | 19,757 | 0.9347× | 0.94× | ✅ |
| socket | 19,583 | 21,067 | 0.9296× | 0.92× | ✅ |
| gzip | 196,022 | 197,083 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,491 | 181,572 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,023 | 4,765 | 1.0540× | 1.05× | ❌ |
| tcp-eof | 5,565 | 10,231 | 0.5439× | 0.54× | ✅ |
| udp | 7,559 | 7,987 | 0.9465× | 1.0× | ✅ |
| connect | 18,254 | 18,707 | 0.9758× | 0.98× | ✅ |
| https | 108,764 | 108,806 | 0.9996× | 1.0× | ✅ |
| wss | 104,620 | 104,794 | 0.9983× | 1.0× | ✅ |
| atomic | 49,671 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,419 | 16,952 | 0.9686× | 0.97× | ✅ |
| http2-concurrent | 361,190 | 408,888 | 0.8833× | 0.89× | ✅ |
| websocket-64k | 156,179 | 159,696 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 13,086 | 16,327 | 0.8015× | 0.79× | ✅ |
| upload | 544,684 | 1,322,380 | 0.4119× | 0.42× | ✅ |
| static | 34,501 | 41,074 | 0.8400× | 0.84× | ✅ |
| pgsql | 23,196 | 24,594 | 0.9432× | 0.95× | ✅ |
| sqlite | 23,478 | 25,232 | 0.9305× | 0.94× | ✅ |
| curl | 85,301 | 90,406 | 0.9435× | 0.95× | ✅ |
| dns | 229,880 | 232,413 | 0.9891× | 1.0× | ✅ |

Geometric mean 0.8942×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
