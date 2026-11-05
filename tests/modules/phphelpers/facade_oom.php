<?php
if (!function_exists('m26_facade_oom')) { fwrite(STDERR, "UNSUPPORTED: matching facade helper required\n"); exit(2); }
m26_facade_oom((int)$argv[1]);
echo "UNREACHABLE: request allocation should exhaust the configured limit\n";
