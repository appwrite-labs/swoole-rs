<?php
// addData/addFile append to the public uploadFiles array; once its next index would
// pass PHP_INT_MAX the append fails and both return false, nothing recorded.
$c = s('client.new', fn() => new Swoole\Coroutine\Http\Client('127.0.0.1', 1));
s('client.addData', fn() => $c->addData('payload', 'field', 'text/plain', 'a.txt'));
s('client.uploads', fn() => $c->uploadFiles);
$c->uploadFiles = [PHP_INT_MAX => 'last'];
s('client.addData', fn() => $c->addData('payload', 'field'));
s('client.addFile', fn() => $c->addFile(__FILE__, 'self'));
s('client.uploads', fn() => $c->uploadFiles);
