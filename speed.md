# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a22c27e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,902 | 15,214 | 0.9138× | 0.92× | ✅ |
| echo-1k | 17,024 | 18,165 | 0.9372× | 0.94× | ✅ |
| json | 22,019 | 23,161 | 0.9507× | 0.95× | ✅ |
| service-health | 16,519 | 17,777 | 0.9292× | 0.93× | ✅ |
| websocket | 6,730 | 8,184 | 0.8223× | 0.82× | ✅ |
| timer | 202,226 | 313,542 | 0.6450× | 0.65× | ✅ |
| app | 125,900 | 135,935 | 0.9262× | 0.93× | ✅ |
| http2 | 19,183 | 20,467 | 0.9373× | 0.94× | ✅ |
| table | 195,723 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,865 | 154,654 | 0.8979× | 0.9× | ✅ |
| task | 31,924 | 33,265 | 0.9597× | 0.96× | ✅ |
| mysql | 49,566 | 51,053 | 0.9709× | 0.97× | ✅ |
| http-client | 33,607 | 35,801 | 0.9387× | 0.94× | ✅ |
| client | 18,471 | 19,757 | 0.9349× | 0.94× | ✅ |
| socket | 19,586 | 21,067 | 0.9297× | 0.92× | ✅ |
| gzip | 196,001 | 197,083 | 0.9945× | 1.0× | ✅ |
| websocket-deflate | 181,490 | 181,572 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,021 | 4,762 | 1.0543× | 1.05× | ❌ |
| tcp-eof | 5,563 | 10,228 | 0.5439× | 0.54× | ✅ |
| udp | 7,557 | 7,987 | 0.9463× | 1.0× | ✅ |
| connect | 18,242 | 18,707 | 0.9752× | 0.98× | ✅ |
| https | 108,797 | 108,813 | 0.9999× | 1.0× | ✅ |
| wss | 104,637 | 104,800 | 0.9984× | 1.0× | ✅ |
| atomic | 49,674 | 51,316 | 0.9680× | 0.97× | ✅ |
| process-mode | 16,407 | 16,963 | 0.9672× | 0.97× | ✅ |
| http2-concurrent | 361,440 | 408,886 | 0.8840× | 0.89× | ✅ |
| websocket-64k | 156,180 | 159,700 | 0.9780× | 0.98× | ✅ |
| websocket-fragmented | 13,083 | 16,331 | 0.8011× | 0.79× | ✅ |
| upload | 544,646 | 1,322,385 | 0.4119× | 0.42× | ✅ |
| static | 34,459 | 41,045 | 0.8395× | 0.84× | ✅ |
| pgsql | 23,188 | 24,595 | 0.9428× | 0.95× | ✅ |
| sqlite | 23,467 | 25,232 | 0.9301× | 0.94× | ✅ |
| curl | 85,302 | 90,381 | 0.9438× | 0.95× | ✅ |
| dns | 230,217 | 232,974 | 0.9882× | 1.0× | ✅ |

Geometric mean 0.8940×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
