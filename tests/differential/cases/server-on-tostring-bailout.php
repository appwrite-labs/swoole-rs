<?php
// An event name whose __toString ends the request: upstream converts it once and stops,
// so the port's on() never converts it a second time.
set_error_handler(null);
$serv = s('server.new', fn() => new Swoole\Server('127.0.0.1', 0, SWOOLE_BASE));
$name = new class {
    function __toString(): string {
        echo "  __toString\n";
        trigger_error('stop', E_USER_ERROR);
        return 'receive';
    }
};
s('server.on', fn() => $serv->on($name, function () {}));
