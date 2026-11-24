<?php
// Requires the SQLite3 ODBC driver. With an unregistered driver, pass its library path as argv[1].
class FailedLob {
    public $context;
    private bool $hasPage;

    public function stream_open($path, $mode, $options, &$openedPath): bool {
        $this->hasPage = $path === 'failedlob://partial';
        return true;
    }

    public function stream_read($count): string|false {
        if ($this->hasPage) {
            $this->hasPage = false;
            return str_repeat('x', $count);
        }
        return false;
    }

    public function stream_eof(): bool {
        return false;
    }

    public function stream_stat(): array {
        return [];
    }
}

stream_wrapper_register('failedlob', FailedLob::class);
$driver = $argv[1] ?? 'SQLite3';
Swoole\Runtime::enableCoroutine();
Swoole\Coroutine\run(function () use ($driver) {
    $db = new PDO("odbc:Driver=$driver;Database=:memory:", null, null, [PDO::ATTR_ERRMODE => PDO::ERRMODE_SILENT]);
    $db->exec('CREATE TABLE lob_test (val BLOB)');
    $insert = $db->prepare('INSERT INTO lob_test VALUES (?)');
    $lob = null;
    $insert->bindParam(1, $lob, PDO::PARAM_LOB);
    foreach (['failedlob://immediate', 'failedlob://partial'] as $path) {
        $lob = fopen($path, 'rb');
        if ($insert->execute()) {
            throw new RuntimeException("Failed read from $path was accepted");
        }
        if ($insert->errorCode() !== 'HY000') {
            throw new RuntimeException('Missing read failure diagnostic');
        }
    }
    $large = str_repeat('page', 600000);
    foreach (['', 'abc', $large] as $bytes) {
        $lob = fopen('php://memory', 'w+b');
        fwrite($lob, $bytes);
        rewind($lob);
        if (!$insert->execute()) {
            throw new RuntimeException('Could not reuse the statement after cancellation');
        }
    }
    $lengths = $db->query('SELECT length(val) FROM lob_test ORDER BY length(val)')->fetchAll(PDO::FETCH_COLUMN);
    if (array_map('intval', $lengths) !== [0, 3, strlen($large)]) {
        throw new RuntimeException('Unexpected stored LOBs');
    }
});
