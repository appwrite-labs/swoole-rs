# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `631c09a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,776 | 15,213 | 0.9055× | 0.91× | ✅ |
| echo-1k | 16,806 | 18,162 | 0.9253× | 0.92× | ✅ |
| json | 22,093 | 23,173 | 0.9534× | 0.95× | ✅ |
| service-health | 16,354 | 17,786 | 0.9195× | 0.92× | ✅ |
| websocket | 6,721 | 8,189 | 0.8208× | 0.81× | ✅ |
| timer | 202,077 | 313,790 | 0.6440× | 0.65× | ✅ |
| app | 127,639 | 135,956 | 0.9388× | 0.93× | ✅ |
| http2 | 18,946 | 20,500 | 0.9242× | 0.92× | ✅ |
| table | 194,323 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,375 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,088 | 33,268 | 0.9645× | 0.96× | ✅ |
| mysql | 50,314 | 51,085 | 0.9849× | 0.97× | ✅ |
| http-client | 33,637 | 35,797 | 0.9397× | 0.93× | ✅ |
| gzip | 194,296 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9524× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,331 | 108,813 | 0.9772× | 0.98× | ✅ |
| wss | 101,595 | 104,800 | 0.9694× | 0.97× | ✅ |
| atomic | 50,209 | 51,276 | 0.9792× | 0.97× | ✅ |
| process-mode | 15,435 | 17,024 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 364,078 | 408,864 | 0.8905× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,372 | 0.7285× | 0.72× | ✅ |
| upload | 416,531 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,861 | 40,999 | 0.6308× | 0.64× | ✅ |
| client | 18,631 | 19,720 | 0.9448× | 0.93× | ✅ |
| socket | 19,873 | 21,037 | 0.9447× | 0.93× | ✅ |
| pgsql | 22,838 | 24,663 | 0.9260× | 0.92× | ✅ |
| sqlite | 23,536 | 25,268 | 0.9315× | 0.93× | ✅ |
| curl | 86,147 | 90,456 | 0.9524× | 0.95× | ✅ |
| dns | 230,128 | 232,846 | 0.9883× | 0.99× | ✅ |
| file | 109,104 | 131,297 | 0.8310× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
