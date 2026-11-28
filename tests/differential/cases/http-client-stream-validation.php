<?php
use Swoole\Coroutine\Http\Client;

Swoole\Coroutine\run(function () {
    $client = new Client('127.0.0.1', 1);
    $source = tmpfile();
    $dir = opendir(__DIR__);
    $writeOnly = fopen('php://memory', 'wb');
    $closed = tmpfile();
    fclose($closed);
    $client->setData('original');
    foreach ([null, 1, true, new stdClass(), stream_context_create(), $dir, $writeOnly, $closed] as $invalid) {
        s('setData', fn() => $client->setData($invalid));
        s('preserved body', fn() => $client->requestBody === 'original');
    }
    foreach (['addData', 'addFile'] as $add) {
        $part = $add === 'addFile' ? __FILE__ : 'content';
        $client->uploadFiles = null;
        $client->setData($source);
        s($add, fn() => $client->$add($part, 'part'));
        s('preserved uploads', fn() => $client->uploadFiles === null);
        $client->setData('original');
        $client->$add($part, 'part');
        s('setData', fn() => $client->setData($source));
        s('post', fn() => $client->post('/', $source));
        s('preserved body', fn() => $client->requestBody === 'original');
        $client->requestBody = $source;
        s('execute', fn() => $client->execute('/'));
        s('connected', fn() => $client->connected);
    }
    $client->uploadFiles = null;
    fclose($source);
    s('execute closed stream', fn() => $client->execute('/'));
    closedir($dir);
    fclose($writeOnly);
});
