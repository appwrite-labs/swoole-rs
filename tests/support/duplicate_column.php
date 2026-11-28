<?php
// Both table witnesses return JSON. Capture the expected Swoole log line separately.
function add_duplicate_column(Swoole\Table $table, string $name, int $type, int $size): bool {
    $log = tempnam(sys_get_temp_dir(), 'table-column-');
    swoole_async_set(['log_file' => $log]);
    register_shutdown_function(static function () use ($log, $name): void {
        $lines = file($log, FILE_IGNORE_NEW_LINES);
        unlink($log);
        if (count($lines) !== 1 || !str_ends_with($lines[0], "\tWARNING\tTable::add_column(): column[$name] already exists")) {
            throw new RuntimeException("Unexpected table diagnostics: " . implode("\n", $lines));
        }
    });
    return $table->column($name, $type, $size);
}
