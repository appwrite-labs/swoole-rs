<?php
function check(bool $condition, string $message): void {
    if (!$condition) { throw new RuntimeException($message); }
}
$summary = m14_macro_profile($argv[1]);
$records = file($argv[1] . '/macro.log');
$labels = ['INFO', 'NOTICE', 'WARNING', 'NOTICE', 'WARNING', 'WARNING'];
if ($summary['trace']) { $labels[] = 'TRACE'; }
if ($summary['debug']) { array_push($labels, ...array_fill(0, 5, 'DEBUG')); }
check(count($records) === count($labels), 'exact accepted record count');
foreach ($records as $i => $record) {
    check(preg_match('/^\[MACRO [^\]]+\]\t' . $labels[$i] . '\t.+\n$/D', $record) === 1, 'ordered labels');
}
$content = implode('', $records);
check(!str_contains($content, '777777') && !str_contains($content, '999999'), 'filtered/ignored records absent');
check(str_contains($content, '(ERRNO 888888): payload'), 'accepted error record');
if ($summary['debug']) {
    $hex = implode(' ', array_map(fn($x) => sprintf('%02x', ord($x)), str_split('0123456789abcdef'))) . ' ';
    check(str_contains($content, '| 00000000 | ' . $hex . '| 0123456789abcdef |'), 'full ASCII hex row');
    check(str_contains($content, '| 00000010 | ' . str_pad('5a ', 48) . '| ' . str_pad('Z', 16) . ' |'), 'partial ASCII hex row');
}
$summary['records'] = count($records);
echo json_encode($summary, JSON_THROW_ON_ERROR), "\n";
