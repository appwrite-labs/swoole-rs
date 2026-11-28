<?php
$summary = m14_storage($argv[1]);
$records = file($argv[1] . '/backtrace.log');
if (count($records) !== 3) { throw new RuntimeException('exact accepted record count'); }
foreach ([['NOTICE', 'default'], ['WARNING', 'enabled'], ['NOTICE', 'reset']] as $index => [$level, $content]) {
    if (preg_match('/^\[TRACE [^\]]+\]\t' . $level . '\t' . $content . '\n$/D', $records[$index]) !== 1) {
        throw new RuntimeException('ordered normal log record');
    }
}
echo json_encode($summary, JSON_THROW_ON_ERROR), "\n";
