<?php
$dsn = 'pgsql:host=' . (getenv('PGSQL_HOST') ?: '127.0.0.1') . ';dbname=test';
if (PHP_VERSION_ID < 80400) {
    $db = new PDO($dsn, 'root', 'root');
    $db->exec('CREATE TEMP TABLE copy_rows (value text)');
    $db->pgsqlCopyFromArray('copy_rows', ['first', 'second']);
    if ($db->pgsqlCopyToArray('copy_rows') !== ["first\n", "second\n"]) {
        throw new RuntimeException('COPY round trip failed');
    }
    return;
}
$db = new Pdo\Pgsql($dsn, 'root', 'root');
$db->exec('CREATE TEMP TABLE copy_rows (value text)');
$db->copyFromArray('copy_rows', ['first', 'second']);
if ($db->copyToArray('copy_rows') !== ["first\n", "second\n"]) {
    throw new RuntimeException('COPY round trip failed');
}
if (PHP_VERSION_ID >= 80500) {
    $rows = (function () {
        yield 'incomplete';
        throw new RuntimeException('row producer failed');
    })();
    try {
        $db->copyFromArray('copy_rows', $rows);
        throw new RuntimeException('COPY lost the generator exception');
    } catch (RuntimeException $e) {
        if ($e->getMessage() !== 'row producer failed') {
            throw $e;
        }
    }
    if ($db->copyToArray('copy_rows') !== ["first\n", "second\n"]) {
        throw new RuntimeException('aborted COPY changed the table');
    }
}
$db->beginTransaction();
$oid = $db->lobCreate();
$stream = $db->lobOpen($oid, 'w+');
if (fwrite($stream, "a\0b") !== 3 || fseek($stream, 1) !== 0 || fread($stream, 2) !== "\0b") {
    throw new RuntimeException('large-object stream round trip failed');
}
fclose($stream);
if (!$db->lobUnlink($oid)) {
    throw new RuntimeException('large-object unlink failed');
}
$db->rollBack();
