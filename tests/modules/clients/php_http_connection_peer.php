<?php
$mode=$argv[1];function check(bool $ok,string $message):void{if(!$ok)throw new RuntimeException($message);}
$server=stream_socket_server('tcp://127.0.0.1:0',$errno,$error);check(is_resource($server),'listen');$address=stream_socket_get_name($server,false);
if($mode==='refused'){fclose($server);echo $address,"\n";flush();echo json_encode(['accepts'=>0],JSON_THROW_ON_ERROR),"\n";exit;}
echo $address,"\n";flush();$count=$mode==='reconnect'?2:1;
for($i=0;$i<$count;++$i){$connection=stream_socket_accept($server,15);check(is_resource($connection),'accept');stream_set_timeout($connection,15);$line=fgets($connection);check(is_string($line)&&str_starts_with($line,'GET /'),'GET request');do{$header=fgets($connection);check(is_string($header),'headers');}while($header!=="\r\n");
 $body=$i===0?'A':'B';$response="HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: keep-alive\r\n\r\n".$body;check(fwrite($connection,$response)===strlen($response),'response');
 if($mode==='reconnect'&&$i===0){fclose($connection);echo "first-closed\n";flush();continue;}
 check(fread($connection,1)===''&&feof($connection),'client EOF');fclose($connection);
}
fclose($server);echo json_encode(['accepts'=>$count,'eof'=>true],JSON_THROW_ON_ERROR),"\n";
