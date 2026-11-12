# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `72bc36e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,928 | 15,210 | 0.9157× | 0.92× | ✅ |
| echo-1k | 17,041 | 18,165 | 0.9381× | 0.94× | ✅ |
| json | 22,020 | 23,161 | 0.9507× | 0.96× | ✅ |
| service-health | 16,516 | 17,773 | 0.9292× | 0.93× | ✅ |
| websocket | 6,715 | 8,191 | 0.8198× | 0.82× | ✅ |
| timer | 202,500 | 313,569 | 0.6458× | 0.65× | ✅ |
| app | 125,915 | 135,854 | 0.9268× | 0.93× | ✅ |
| http2 | 19,816 | 20,464 | 0.9683× | 0.97× | ✅ |
| table | 195,729 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,876 | 154,654 | 0.8980× | 0.9× | ✅ |
| task | 31,933 | 33,265 | 0.9600× | 0.96× | ✅ |
| mysql | 49,567 | 51,061 | 0.9707× | 0.97× | ✅ |
| http-client | 33,587 | 35,809 | 0.9379× | 0.94× | ✅ |
| client | 18,473 | 19,757 | 0.9350× | 0.94× | ✅ |
| socket | 19,752 | 21,067 | 0.9376× | 0.92× | ✅ |
| gzip | 196,003 | 197,079 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,477 | 181,572 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,023 | 4,765 | 1.0540× | 1.05× | ❌ |
| tcp-eof | 5,565 | 10,231 | 0.5439× | 0.54× | ✅ |
| udp | 8,038 | 7,986 | 1.0065× | 1.0× | ❌ |
| connect | 18,248 | 18,707 | 0.9755× | 0.98× | ✅ |
| https | 108,839 | 108,806 | 1.0003× | 1.01× | ➖ |
| wss | 104,669 | 104,794 | 0.9988× | 1.0× | ✅ |
| atomic | 49,680 | 51,312 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,436 | 16,957 | 0.9693× | 0.97× | ✅ |
| http2-concurrent | 372,273 | 408,880 | 0.9105× | 0.91× | ✅ |
| websocket-64k | 156,166 | 159,699 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,079 | 16,327 | 0.8011× | 0.79× | ✅ |
| upload | 544,681 | 1,322,384 | 0.4119× | 0.42× | ✅ |
| static | 34,478 | 41,074 | 0.8394× | 0.84× | ✅ |
| pgsql | 23,228 | 24,595 | 0.9444× | 0.95× | ✅ |
| sqlite | 23,493 | 25,232 | 0.9311× | 0.94× | ✅ |
| curl | 85,332 | 90,509 | 0.9428× | 0.95× | ✅ |
| dns | 231,085 | 233,198 | 0.9909× | 1.0× | ✅ |

Geometric mean 0.8978×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
