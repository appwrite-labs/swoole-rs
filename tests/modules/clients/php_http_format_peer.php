<?php
function check(bool $ok,string $message):void{if(!$ok)throw new RuntimeException($message);}
$server=stream_socket_server('tcp://127.0.0.1:0',$errno,$error);check(is_resource($server),'listen');echo stream_socket_get_name($server,false),"\n";flush();$connection=stream_socket_accept($server,15);check(is_resource($connection),'accept');stream_set_timeout($connection,15);
$request=rtrim((string)fgets($connection),"\r\n");$headers=[];$length=0;
while(true){$line=fgets($connection);check(is_string($line),'header EOF');if($line==="\r\n")break;$line=rtrim($line,"\r\n");$headers[]=$line;if(str_starts_with(strtolower($line),'content-length:'))$length=(int)trim(substr($line,15));}
check($length>=0&&$length<=16384,'bounded fixture request');$body='';while(strlen($body)<$length){$chunk=fread($connection,$length-strlen($body));check(is_string($chunk)&&$chunk!=='','body EOF');$body.=$chunk;}
$response="HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nOK";check(fwrite($connection,$response)===strlen($response),'reply');check(fread($connection,1)===''&&feof($connection),'client closes nonkeepalive request');fclose($connection);fclose($server);echo json_encode(['request'=>$request,'headers'=>$headers,'content_length'=>$length,'body_hex'=>bin2hex($body),'eof'=>true],JSON_THROW_ON_ERROR),"\n";
