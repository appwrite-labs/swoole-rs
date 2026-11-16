<?php
$summary = m14_storage($argv[1]);
if ($summary !== ['gtest_exit' => 0, 'original_cases' => (int) $argv[2]]) {
    throw new RuntimeException('original GoogleTest failure');
}
echo json_encode($summary, JSON_THROW_ON_ERROR), "\n";
