<?php
if (!function_exists('m26_facade_oom')) { fwrite(STDERR, "UNSUPPORTED: matching facade helper required\n"); exit(2); }
$pressure = str_repeat('p', isset($argv[2]) ? (int)$argv[2] : 0);
m26_facade_oom((int)$argv[1]);
echo "UNREACHABLE: request allocation should exhaust the configured limit\n";
