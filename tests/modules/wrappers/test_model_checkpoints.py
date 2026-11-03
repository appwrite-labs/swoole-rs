"""Python-only model-runner checkpoint tests; no native build or stopped probe."""
from pathlib import Path
import contextlib
import io
import json
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE))
import response


class Checkpoints(unittest.TestCase):
    def test_initial_and_mid_sdk_guard_keep_partial_report(self):
        method='bool Context::end(const char *data, size_t length) {\n}\n'
        formatter='size_t sw_snprintf() {return 0;}\nsize_t sw_vsnprintf'
        def original(path):return formatter if path=='src/core/base.cc'else method
        for stop in (1,3):
            with self.subTest(stop=stop),tempfile.TemporaryDirectory() as directory:
                root=Path(directory);archive=root/'inert.a';archive.write_bytes(b'no native archive')
                core=root/'sdk';core.mkdir();(core/'config.h').write_text('inert metadata')
                hybrid=root/'hybrid';(hybrid/'src/wrapper').mkdir(parents=True);(hybrid/'src/wrapper/http.cc').write_text(method);(hybrid/'src/wrapper/event.cc').write_text('inert')
                output=root/'result.json';calls=[]
                def guard(_):
                    calls.append(None)
                    if len(calls)==stop:raise RuntimeError('synthetic runner interruption')
                class Result:
                    returncode=0;stdout='inert subprocess fixture';stderr=''
                def run(command,**kwargs):
                    if '-o'in command:Path(command[command.index('-o')+1]).write_bytes(b'inert executable metadata')
                    return Result()
                argv=[str(HERE/'response.py'),'--source',str(root),'--archive',str(archive),'--core',str(core),'--output',str(output)]
                with patch.object(sys,'argv',argv),patch.object(response,'hybrid_tree',return_value=(hybrid,[])),patch.object(response.overlay,'original',side_effect=original),patch.object(response,'run_isolated',side_effect=run),patch.object(response,'require_build_space',side_effect=guard),contextlib.redirect_stdout(io.StringIO()):
                    with self.assertRaisesRegex(RuntimeError,'synthetic runner interruption'):response.main()
                data=json.loads(output.read_text());self.assertFalse(data['passed']);self.assertEqual(data['execution_status'],'blocked_build_space');self.assertEqual(data['error'],'synthetic runner interruption')
                self.assertEqual('run'in data,stop>1);self.assertEqual(len(data['sdk']),max(0,stop-2))


if __name__=='__main__':unittest.main()
