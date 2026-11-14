<?php
// Nonempty binary column names use the pinned std::string(name, length) API.
// No callbacks, reentry, invalidation, overflow or missing-row access.
$checks = 0;
function check_column_bytes($actual, $expected, string $label): void {
    global $checks;
    if ($actual !== $expected) {
        throw new RuntimeException($label . ': ' . var_export($actual, true) . ' != ' . var_export($expected, true));
    }
    ++$checks;
}
$table = new Swoole\Table(16);
$names = ['plain', "plain\0tail", "plain\0other", "high_\xff"];
$values = [];
foreach ($names as $index => $name) {
    check_column_bytes($table->column($name, Swoole\Table::TYPE_INT, 8), true, 'binary column declaration');
    $values[$name] = $index + 11;
}
check_column_bytes($table->create(), true, 'binary schema create');
check_column_bytes($table->set('row', array_reverse($values, true)), true, 'binary field input');
check_column_bytes($table->get('row'), $values, 'materialized keys preserve byte length and insertion order');
foreach ($values as $name => $value) {
    check_column_bytes($table->get('row', $name), $value, 'exact binary lookup');
}
check_column_bytes($table->set('row', ["plain\0tail" => 91]), true, 'binary field update');
$values["plain\0tail"] = 91;
check_column_bytes($table->get('row'), $values, 'binary update preserves prefix-sharing fields');
check_column_bytes($table->count(1), 4, 'binary declaration count');
$rows = [];
foreach ($table as $key => $row) { $rows[$key] = $row; }
check_column_bytes($rows, ['row' => $values], 'binary iterator materialization');
check_column_bytes($table->destroy(), true, 'binary table teardown');
echo json_encode(['checks' => $checks]), "\n";
