"""Optional discovery failures cannot discard available required dependency flags."""
import importlib.util
import itertools
from pathlib import Path
import shlex
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch
sys.path.insert(0,str(Path(__file__).resolve().parent))
spec=importlib.util.spec_from_file_location('phphelpers_sdk',Path(__file__).with_name('sdk.py'))
sdk=importlib.util.module_from_spec(spec);spec.loader.exec_module(sdk)


class CompilationMatrix(unittest.TestCase):
    def report(self):
        report={'feature_profiles':{'native':[],'hybrid':['SWOOLE_RS_PHPHELPERS']},
                'sources':['ext-src/php_swoole.cc','ext-src/php_swoole_cxx.cc','ext-src/swoole_stdext.cc','ext-src/php_swoole_cxx.h'],
                'cpp_standards':['c++14','c++17']}
        report['commands']=[dict(engine=engine,stdext_enabled=enabled,unit=unit,cpp_standard=standard,returncode=0)
                            for engine,enabled,unit,standard in itertools.product(
                                report['feature_profiles'],[False,True],report['sources'][:3],report['cpp_standards'])]
        return report

    def test_complete_expanded_matrix(self):
        report=self.report()
        self.assertEqual(len(report['commands']),24)
        self.assertTrue(sdk.compilation_matrix_passed(report))

    def test_missing_duplicate_extra_failed_and_wrong_profile_rejected(self):
        for defect in ['missing','duplicate','extra','failed','wrong_profile']:
            with self.subTest(defect=defect):
                report=self.report()
                if defect=='missing':report['commands'].pop()
                elif defect=='duplicate':report['commands'][-1]=dict(report['commands'][0])
                elif defect=='extra':report['commands'].append(dict(report['commands'][0]))
                elif defect=='failed':report['commands'][0]['returncode']=1
                else:report['commands'][0]['stdext_enabled']='False'
                self.assertFalse(sdk.compilation_matrix_passed(report))

    def test_empty_inventory_rejected(self):
        report=self.report();report['sources']=[];report['commands']=[]
        self.assertFalse(sdk.compilation_matrix_passed(report))


class DependencyFlags(unittest.TestCase):
    def test_optional_failure_retains_required_flags_and_error(self):
        packages={'openssl':(0,'-I/ssl/include\n',''),
                  'libpcre2-8':(0,'-I/pcre/include\n',''),
                  'libcurl':(0,'-I"/curl sdk/include"\n','curl warning\n'),
                  'libpq':(1,'','Package libpq was not found\n')}
        def probe(command,**options):
            self.assertEqual(command[:2],['pkg-config','--cflags'])
            code,out,err=packages[command[2]]
            return SimpleNamespace(returncode=code,stdout=out,stderr=err)
        with patch.object(sdk.subprocess,'run',side_effect=probe) as run:
            result=sdk.dependency_flags()
        self.assertEqual(run.call_count,4)
        self.assertEqual(shlex.split(result['stdout']),['-I/ssl/include','-I/pcre/include','-I/curl sdk/include'])
        self.assertEqual(result['returncode'],1)
        self.assertEqual(result['probes']['libpq']['returncode'],1)
        self.assertIn('Package libpq was not found',result['stderr'])
        self.assertEqual(result['probes']['libcurl']['stderr'],'curl warning\n')

    def test_missing_program_keeps_each_os_error(self):
        with patch.object(sdk.subprocess,'run',side_effect=FileNotFoundError('pkg-config missing')):
            result=sdk.dependency_flags()
        self.assertEqual(result['returncode'],1)
        self.assertFalse(result['stdout'].strip())
        self.assertEqual(len(result['probes']),4)
        for row in result['probes'].values():
            self.assertIsNone(row['returncode'])
            self.assertEqual(row['error_type'],'FileNotFoundError')
            self.assertIn('pkg-config missing',row['stderr'])



if __name__=='__main__':unittest.main()
