"""Keep legacy helper compilation and reject mixed selected-header identities."""
import hashlib
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
sys.path.insert(0,str(Path(__file__).resolve().parent))
import entrypoint


class HelperHeaderIdentity(unittest.TestCase):
    def setup_sdk(self,root,profile='native',consumed=b'native'):
        native=root/'headers-native/ext-src/php_swoole_cxx.h';native.parent.mkdir(parents=True);native.write_bytes(b'native')
        header=root/'consumed/php_swoole_cxx.h';header.parent.mkdir();header.write_bytes(consumed)
        sdk={'passed':True,'pin':entrypoint.PIN,'work':str(root),'php_version':'8.5.test',
             'mirror_sha256':{'ext-src/php_swoole_cxx.h':entrypoint.sha(native)},
             'commands':[{'engine':profile,'stdext_enabled':False,'cpp_standard':'c++14','unit':'ext-src/php_swoole_cxx.cc',
                 'dependencies_sha256':{str(header):entrypoint.sha(header)},'command':['c++','-fsyntax-only','-std=c++14','-MD','-MF','old.d','old.cc']}]}
        path=root/'sdk.json';path.write_text(json.dumps(sdk));return path,sdk,native,header

    def test_legacy_native_profile_keeps_compilation_and_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);path,sdk,native,header=self.setup_sdk(root)
            work=root/'work';work.mkdir();(work/'entrypoint.cc').write_text('source')
            def compile_helper(command,timeout):
                self.assertEqual(command[:2],['c++','-std=c++14'])
                self.assertNotIn('-fsyntax-only',command);self.assertIn('-O2',command)
                (work/'m26_helpers.so').write_bytes(b'module')
                (work/'entrypoint.d').write_text('module: '+str(header)+'\n')
                return SimpleNamespace(returncode=0,stdout='',stderr='')
            report={}
            with patch.object(entrypoint,'PUBLIC_HEADER_SHA256',entrypoint.sha(native)),patch.object(entrypoint,'require_build_space'),patch.object(entrypoint,'run_isolated',side_effect=compile_helper):
                result=entrypoint.build_helper(path,work,report)
            self.assertEqual(result.read_bytes(),b'module');self.assertEqual(report['helper_compilation']['sdk_profile'],'native')
            self.assertEqual(report['helper_compilation']['dependencies_sha256'],{str(header.resolve()):entrypoint.sha(header)})

    def test_passing_sdk_cannot_substitute_different_selected_header(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);path,sdk,native,header=self.setup_sdk(root,'hybrid',b'selected actual')
            sdk.update(selected_header_sha256=hashlib.sha256(b'other selected').hexdigest());path.write_text(json.dumps(sdk))
            with patch.object(entrypoint,'PUBLIC_HEADER_SHA256',entrypoint.sha(native)),patch.object(entrypoint,'run_isolated') as run:
                with self.assertRaisesRegex(ValueError,'Selected public header differs'):
                    entrypoint.build_helper(path,root,{},profile='hybrid')
            run.assert_not_called()


if __name__=='__main__':unittest.main()
