<?php
function check(bool $condition, string $message): void {
    if (!$condition) { throw new RuntimeException($message); }
}
$root = $argv[1];
$summary = m14_storage($root);
check($summary['configuration_formats'] === 10 && $summary['configuration_accepted'] === 7 &&
      $summary['configuration_rejected'] === 3 && $summary['configuration_resets'] === 32, 'ordinary configuration cases complete');
check($summary['level_cases'] === 11 && $summary['filename_cases'] === 54, 'native cases complete');
$old = file_get_contents("$root/old-file");
$new = file_get_contents("$root/" . str_repeat('f', 180));
check(preg_match('/^\[STORE [^\]]+\]\tNOTICE\tbefore\n$/D', $old) === 1, 'old FILE receives before');
check(preg_match('/^\[STORE [^\]]+\]\tNOTICE\tafter\n$/D', $new) === 1, 'reopened FILE receives after');
$rotated = glob("$root/reset-file.*");
check(count($rotated) === 1 && !file_exists("$root/reset-file"), 'reset preserves rotated FILE');
check(preg_match('/^\[\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} [^\]]+\]\tNOTICE\treset\n$/D',
    file_get_contents($rotated[0])) === 1, 'reset restores date and disables microseconds');
$records = file("$root/concurrent");
check(count($records) === 1024, 'every concurrent record retained');
foreach ($records as $number => $record) {
    check(preg_match('/^\[REOPEN [^\]]+\]\tNOTICE\trecord-' . $number . '\n$/D', $record) === 1,
        'exact ordered record across actual reopen');
}
check($summary['thread_records'] === 1024 && $summary['thread_reopens'] === 64, 'thread workloads complete');
echo json_encode($summary, JSON_THROW_ON_ERROR), "\n";
