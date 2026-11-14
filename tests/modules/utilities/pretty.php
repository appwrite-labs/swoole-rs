<?php
$summary = m14_storage($argv[1]);
if ($summary !== ['pretty_cases' => 42096, 'colon_names' => 1040, 'strip_modes' => 2]) {
    throw new RuntimeException('exact actual public pretty-name cases');
}
echo json_encode($summary, JSON_THROW_ON_ERROR), "\n";
