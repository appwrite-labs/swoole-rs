<?php
$mode=$argv[1];function check(bool $ok,string $message):void{if(!$ok)throw new RuntimeException($message);}
$server=stream_socket_server('tcp://127.0.0.1:0',$errno,$error);check(is_resource($server),'listen');echo stream_socket_get_name($server,false),"\n";flush();$connection=stream_socket_accept($server,15);check(is_resource($connection),'accept');stream_set_timeout($connection,15);
if($mode==='timeout'){check(fread($connection,1)===''&&feof($connection),'timeout client EOF');$summary=['accepted'=>1,'client_eof'=>true];}
else {
 $request='';$size=$mode==='sendfile'?5:9;while(strlen($request)<$size){$chunk=fread($connection,$size-strlen($request));check(is_string($chunk)&&$chunk!=='','request');$request.=$chunk;}
 check($request===($mode==='sendfile'?"F\0ILE":"raw\0data\n"),'exact request bytes');
 $response=match($mode){'length'=>pack('N',5)."A\0BCD".pack('N',3)."M\0N",'eof'=>"A\0B\r\n",'sendfile'=>'OK',default=>"REPLY\0OK"};check(fwrite($connection,$response)===strlen($response),'response');$summary=['accepted'=>1,'request_hex'=>bin2hex($request),'response_hex'=>bin2hex($response),'peer_fin'=>true];
}
fclose($connection);fclose($server);echo json_encode($summary,JSON_THROW_ON_ERROR),"\n";
