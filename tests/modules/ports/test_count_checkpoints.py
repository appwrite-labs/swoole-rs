"""Runner-only interruption checks; no native code, fault probes or builds execute."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE))
import count


class Checkpoints(unittest.TestCase):
    def test_preserve_completed_work_before_guard_stop(self):
        for stop in (1,3):
            with self.subTest(stop=stop),tempfile.TemporaryDirectory() as directory:
                root=Path(directory);build=root/'hybrid';archive=build/'rust-prefix/lib/test.a';archive.parent.mkdir(parents=True);archive.write_bytes(b'inert metadata fixture')
                (build/'rust-prefix/include').mkdir();(build/'src/include').mkdir(parents=True);(build/'src/src/server').mkdir(parents=True);(build/'src/config.h').write_text('')
                (build/'src/include/swoole.h').write_text('');(build/'src/src/server/master.cc').write_text('size_t ListenPort::get_connection_num() const {\n}\n')
                core=root/'sdk';(core/'include').mkdir(parents=True)
                for p in [core/'config.h',core/'include/swoole_server.h',core/'include/swoole_atomic.h']:p.write_text('inert header metadata')
                output=root/'report.json';calls=[]
                def guard(_):
                    calls.append(None)
                    if len(calls)==stop:raise RuntimeError('synthetic runner interruption')
                class Result:
                    returncode=0;stdout='inert subprocess fixture';stderr=''
                def runner(command,**kwargs):
                    if '-o'in command:Path(command[command.index('-o')+1]).write_bytes(b'inert model metadata')
                    return Result()
                argv=[str(HERE/'count.py'),'--source',str(root),'--archive',str(archive),'--core',str(core),'--output',str(output)]
                with patch.object(sys,'argv',argv),patch.object(count.overlay,'original',return_value='size_t ListenPort::get_connection_num() const {\n}\n'),patch.object(count,'require_build_space',side_effect=guard),patch.object(count,'run_isolated',side_effect=runner),patch.object(count,'configured_flags',return_value=([],{})),patch.object(count.subprocess,'check_output',return_value=''),contextlib.redirect_stdout(io.StringIO()):
                    with self.assertRaisesRegex(RuntimeError,'synthetic runner interruption'):count.main(['size_t ListenPort::get_connection_num() const'],HERE/'defaults.cc')
                data=json.loads(output.read_text());self.assertFalse(data['passed']);self.assertEqual(data['execution_status'],'blocked_build_space')
                self.assertEqual(data['error'],'synthetic runner interruption')
                self.assertEqual(len(data['sdk']),max(0,stop-2))
                self.assertEqual('run'in data,stop>1)


if __name__=='__main__':unittest.main()
