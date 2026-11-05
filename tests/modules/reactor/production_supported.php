<?php
$result = swr_reactor_supported();
$cases = PHP_OS_FAMILY === 'Darwin' ? 14 : 16;
if ($result['cases'] !== $cases || count($result['outcomes']) !== $cases * 15 || in_array(false, $result['outcomes'], true)) {
    throw new RuntimeException('Supported positive Reactor assertions failed: ' . json_encode($result));
}
if (count($result['owner_outcomes']) !== $cases * 4 || in_array(false, $result['owner_outcomes'], true)) {
    throw new RuntimeException('Reactor public value/address semantics failed: ' . json_encode($result));
}
$result['owner_checks'] = count($result['owner_outcomes']);
unset($result['owner_outcomes']);
$result['checks'] = count($result['outcomes']);
unset($result['outcomes']);
echo json_encode($result), "\n";
