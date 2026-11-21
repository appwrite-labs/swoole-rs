<?php
require __DIR__ . '/../bootstrap.php';
require __DIR__ . '/Store.php';
require __DIR__ . '/Api.php';
// Open the database in the worker, so no PDO connection is shared across fork().
$api = null;
$server = httpServer();
$server->on('request', static function ($request, $response) use (&$api): void {
    $api ??= new Api(new Store(getenv('TEST_DATA') . '/jobs.sqlite'));
    $api($request, $response);
});
$server->start();
