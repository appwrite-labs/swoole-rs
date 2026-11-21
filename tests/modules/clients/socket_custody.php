<?php
function checkCustody(bool $condition, string $message): void {
    if (!$condition) { throw new RuntimeException($message); }
}
$summary=[];
Swoole\Coroutine\run(function () use (&$summary): void {
    foreach (['release-owner', 'close-owner', 'release-stream'] as $mode) {
        $pair=stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, 0);
        checkCustody(is_array($pair), 'native stream socket pair');
        $socket=Swoole\Coroutine\Socket::import($pair[0]);
        checkCustody($socket instanceof Swoole\Coroutine\Socket, 'import creates socket object');
        checkCustody($socket->send("A\0B")===3 && fread($pair[1],3)==="A\0B", 'import preserves binary transport');
        $weak=WeakReference::create($socket);
        if ($mode==='release-owner') {
            unset($socket);gc_collect_cycles();
            checkCustody($weak->get()===null && is_resource($pair[0]), 'socket GC retains independently owned stream');
            checkCustody(fwrite($pair[0],'next')===4 && fread($pair[1],4)==='next', 'independent stream remains usable');
            fclose($pair[0]);
        } elseif ($mode==='close-owner') {
            checkCustody($socket->close()===true && $socket->isClosed(), 'imported socket close state');
            checkCustody(!is_resource($pair[0]), 'imported close invalidates stream resource');
            unset($socket);gc_collect_cycles();checkCustody($weak->get()===null, 'closed owner collected');
        } else {
            unset($pair[0]);
            checkCustody($socket->send('held')===4 && fread($pair[1],4)==='held', 'socket retains imported stream after original zval release');
            unset($socket);gc_collect_cycles();checkCustody($weak->get()===null, 'last owner collected');
        }
        checkCustody(fread($pair[1],1)==='', 'final owner closes fd and peer sees EOF');
        fclose($pair[1]);$summary[$mode]=true;
    }
});
Swoole\Runtime::enableCoroutine(SWOOLE_HOOK_TCP | SWOOLE_HOOK_UDP | SWOOLE_HOOK_UNIX | SWOOLE_HOOK_STREAM_FUNCTION);
// The registered factory delegates to PHP outside a coroutine.
$server=stream_socket_server('tcp://127.0.0.1:0',$errno,$errstr);
checkCustody(is_resource($server), 'original factory remains available outside coroutine');
fclose($server);$summary['original-factory']=true;
Swoole\Coroutine\run(function () use (&$summary): void {
    for ($round=0;$round<8;++$round) {
        $pair=stream_socket_pair(STREAM_PF_UNIX,STREAM_SOCK_STREAM,0);
        checkCustody(is_array($pair), 'hooked stream pair allocation');
        checkCustody(stream_set_blocking($pair[0],false), 'hooked stream nonblocking option');
        // The pinned stream adapter reports blocked=true after this successful option.
        checkCustody(stream_get_meta_data($pair[0])['blocked']===true, 'original hooked stream metadata after nonblocking option');
        checkCustody(stream_set_blocking($pair[0],true), 'restore blocking option');
        $payload="round:$round\0end";
        checkCustody(fwrite($pair[0],$payload)===strlen($payload), 'hooked stream binary write');
        checkCustody(fread($pair[1],strlen($payload))===$payload, 'hooked stream binary read');
        fclose($pair[0]);checkCustody(fread($pair[1],1)==='', 'hooked close peer EOF');fclose($pair[1]);
    }
    $summary['hooked-pairs']=8;
});
Swoole\Runtime::enableCoroutine(false);
echo json_encode($summary,JSON_THROW_ON_ERROR),"\n";
