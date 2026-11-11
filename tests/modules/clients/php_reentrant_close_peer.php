<?php
function check(bool $condition,string $message):void { if(!$condition)throw new RuntimeException($message); }
function exchange($socket,string $expected,string $response):void {
    $bytes='';while(strlen($bytes)<strlen($expected)){$chunk=fread($socket,strlen($expected)-strlen($bytes));check($chunk!==false&&$chunk!=='','read');$bytes.=$chunk;}
    check($bytes===$expected,'exact request');check(fwrite($socket,$response)===strlen($response),'write');
}
function drain($socket):void {
    while(!feof($socket)){$bytes=fread($socket,1024);check($bytes!==false&&!stream_get_meta_data($socket)['timed_out'],'EOF timeout');check($bytes==='','unexpected extra bytes');}
    fclose($socket);
}
$server=stream_socket_server('tcp://127.0.0.1:0',$errno,$error);check($server!==false,'listen');
echo stream_socket_get_name($server,false),"\n";flush();
$accepts=0;
foreach([["one\n",pack('N',3).'ONE'],["two\n","TWO\n"]] as [$request,$response]){
    $socket=stream_socket_accept($server,10);check($socket!==false,'accept');stream_set_timeout($socket,10);$accepts++;
    exchange($socket,$request,$response);drain($socket);
}
fclose($server);
echo json_encode(['accepts'=>$accepts,'eof'=>true],JSON_THROW_ON_ERROR),"\n";
