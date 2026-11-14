<?php
function check(bool $ok,string $message):void {if(!$ok)throw new RuntimeException($message);}
$server=stream_socket_server('tcp://127.0.0.1:0',$errno,$error);check(is_resource($server),'listen');echo stream_socket_get_name($server,false),"\n";flush();
$connection=stream_socket_accept($server,15);check(is_resource($connection),'accept');stream_set_timeout($connection,15);
foreach(['one','child','parent'] as $word){check(fgets($connection)===$word."\n",'request order:'.$word);check(fwrite($connection,strtoupper($word)."\n")===strlen($word)+1,'response');}
check(fread($connection,1)===''&&feof($connection),'final parent EOF');fclose($connection);fclose($server);echo json_encode(['accepts'=>1,'requests'=>3,'eof'=>true],JSON_THROW_ON_ERROR),"\n";
