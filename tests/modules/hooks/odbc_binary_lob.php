<?php
// The PHPT environment registers mysql-test with MariaDB Connector/ODBC.
$db = new PDO($argv[1] ?? 'odbc:mysql-test', null, null, [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]);
$db->exec('CREATE TEMPORARY TABLE lob_digest (id INT PRIMARY KEY, val LONGBLOB)');
$insert = $db->prepare('INSERT INTO lob_digest VALUES (?, ?)');
$insert->bindParam(1, $id, PDO::PARAM_INT);
$insert->bindParam(2, $lob, PDO::PARAM_LOB);
foreach (['', str_repeat("a\0b\xff", 600000)] as $id => $bytes) {
    $lob = fopen('php://temp', 'w+b');
    fwrite($lob, $bytes);
    rewind($lob);
    $insert->execute();
    $hash = $db->query("SELECT SHA2(val, 256) FROM lob_digest WHERE id = $id")->fetchColumn();
    if ($hash !== hash('sha256', $bytes)) {
        throw new RuntimeException("Stored binary LOB differs for row $id");
    }
    fclose($lob);
}
