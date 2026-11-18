<?php
$dsn = 'pgsql:host=' . (getenv('PGSQL_HOST') ?: '127.0.0.1') . ';dbname=test';
$db = PHP_VERSION_ID >= 80400 ? new Pdo\Pgsql($dsn, 'root', 'root') : new PDO($dsn, 'root', 'root');
$pidMethod = PHP_VERSION_ID >= 80400 ? 'getPid' : 'pgsqlGetPid';
$notifyMethod = PHP_VERSION_ID >= 80400 ? 'getNotify' : 'pgsqlGetNotify';
$pid = $db->$pidMethod();
if ($pid !== (int) $db->query('SELECT pg_backend_pid()')->fetchColumn()) {
    throw new RuntimeException('unexpected backend PID');
}
$db->exec('LISTEN swoole_notify');
if ($db->$notifyMethod(PDO::FETCH_ASSOC, 1) !== false) {
    throw new RuntimeException('empty notification queue');
}
foreach ([PDO::FETCH_ASSOC, PDO::FETCH_NUM, PDO::FETCH_BOTH] as $mode) {
    foreach (['', 'message'] as $payload) {
        $db->exec('NOTIFY swoole_notify, ' . $db->quote($payload));
        $numeric = ['swoole_notify', $pid];
        $named = ['message' => 'swoole_notify', 'pid' => $pid];
        if ($payload !== '') {
            $numeric[] = $named['payload'] = $payload;
        }
        $expected = $mode === PDO::FETCH_NUM ? $numeric : ($mode === PDO::FETCH_ASSOC ? $named : $numeric + $named);
        if ($db->$notifyMethod($mode, 1000) !== $expected) {
            throw new RuntimeException('unexpected notification fields or order');
        }
    }
}
if (PHP_VERSION_ID >= 80400) {
    $messages = [];
    $db->setNoticeCallback(function ($message) use (&$messages) {
        $messages[] = $message;
    });
    $db->exec("DO \$\$ BEGIN RAISE NOTICE 'first notice'; END \$\$");
    $db->setNoticeCallback(null);
    $db->exec("DO \$\$ BEGIN RAISE NOTICE 'second notice'; END \$\$");
    if (count($messages) !== 1 || !str_contains($messages[0], 'first notice')) {
        throw new RuntimeException('notice callback registration or removal failed');
    }
}
