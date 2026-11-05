"""Generate native typed loans from Rust-owned process records."""
from pathlib import Path
import argparse
import re
# Generate checked native views from the canonical Rust fields. Opaque pointer
# refinements preserve the native embedding API, without defining storage twice.
renames={'is_async':'async','protocol_type':'protocol_type_','kind':'type','max_packet_size':'max_packet_size_','registry':'registry_','ipc':'ipc_'}
callbacks={'on_start':'void (*onStart)(ProcessPool *)','on_shutdown':'void (*onShutdown)(ProcessPool *)','on_before_reload':'void (*onBeforeReload)(ProcessPool *)','on_after_reload':'void (*onAfterReload)(ProcessPool *)','on_task':'int (*onTask)(ProcessPool *, Worker *, EventData *)','on_worker_start':'void (*onWorkerStart)(ProcessPool *, Worker *)','on_message':'void (*onMessage)(ProcessPool *, RecvData *)','on_worker_exit':'void (*onWorkerExit)(ProcessPool *, Worker *)','on_worker_stop':'void (*onWorkerStop)(ProcessPool *, Worker *)','on_worker_error':'void (*onWorkerError)(ProcessPool *, Worker *, const ExitStatus &)','on_worker_message':'void (*onWorkerMessage)(ProcessPool *, EventData *)','on_worker_not_found':'int (*onWorkerNotFound)(ProcessPool *, const ExitStatus &)','main_loop':'int (*main_loop)(ProcessPool *, Worker *)'}
types={'bool':'bool','u8':'uint8_t','u16':'uint16_t','u32':'uint32_t','u64':'uint64_t','i64':'int64_t','usize':'size_t','c_int':'int','*mut c_void':'void *','*mut WorkerRecord':'Worker *','*mut WorkerRegistry':'swr_v1_process_worker_registry *','*mut PoolIpc':'swr_v1_process_pool_ipc *','*mut ReloadTask':'swr_v1_process_reload_task *'}
refinements={'pool':'ProcessPool *','queue':'MsgQueue *','lock':'Mutex *','pipe_object':'UnixSocket *','pipe_master':'network::Socket *','pipe_worker':'network::Socket *','pipe_current':'network::Socket *','pipes':'std::vector<std::shared_ptr<UnixSocket>> *','message_box':'Channel *','message_bus':'MessageBus *','round_id':'sw_atomic_t','protocol_type':'ProtocolType','dispatch_count':'sw_atomic_long_t','request_count':'sw_atomic_long_t','response_count':'sw_atomic_long_t','start_time':'time_t','reload_last_time':'time_t','warning_time':'time_t','msgqueue_key':'key_t','exit_time':'time_t'}
text='#ifndef SWR_PROCESS_NATIVE_VIEW_CONTEXT\n#include "swoole_process_pool.h"\n#else\n#ifndef SWOOLE_RS_PROCESS_VIEWS_H\n#define SWOOLE_RS_PROCESS_VIEWS_H\n// Generated from crates/process/src/{worker_record,process_pool}.rs.\n// Typed native loans only; Rust owns the record layouts and operations.\n'
for record,file,view in [('WorkerGlobalRecord','worker_globals','SWR_WORKER_GLOBAL_RECORD_FIELDS'),('WorkerRecord','worker_record','SWR_WORKER_RECORD_FIELDS'),('PoolRecord','process_pool','SWR_POOL_RECORD_FIELDS')]:
 s=Path('crates/process/src/'+file+'.rs').read_text().split('pub struct '+record+' {',1)[1].split('\n}',1)[0]
 fields=re.findall(r'^    (?:pub )?(\w+): ([^,]+),',s,re.M)
 text+='#define '+view+' '+chr(92)+chr(10)
 for name,ty in fields:
  if name in callbacks:decl=callbacks[name]
  else:decl=refinements.get(name,types.get(ty,ty))+' '+renames.get(name,name)
  if record=='PoolRecord' and name=='registry':decl='union { swr_v1_process_worker_registry *registry_; swr_v1_process_worker_registry *map_; }'
  if record=='PoolRecord' and name in ['message_box','message_bus','reload_task']:decl+=' = nullptr'
  text+='    '+decl+';'+(' '+chr(92)+chr(10) if name!=fields[-1][0] else chr(10))
text += '#endif\n#endif\n'
output = Path('native/process/swoole_rs_process_views.h')
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--check', action='store_true')
args = parser.parse_args()
if args.check:
    if not output.is_file() or output.read_text() != text:
        raise SystemExit(f'{output} is stale; run python3 tools/process_records.py')
else:
    output.write_text(text)
