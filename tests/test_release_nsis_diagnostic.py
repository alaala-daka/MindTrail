from pathlib import Path
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'tests')); sys.path.insert(0,str(ROOT/'scripts'))
from release_nsis_diagnostic import transform_script

class NsisDiagnosticTests(unittest.TestCase):
    def test_only_fixed_safe_transform(self):
        text='!define UNINSTALLERSIGNCOMMAND ""\n!uninstfinalize \'${UNINSTALLERSIGNCOMMAND}\'\n!define OUTFILE "old"\n'
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            result=transform_script(text,Path(tmp)/'synthetic.exe')
            self.assertIn('synthetic.exe',result); self.assertNotIn('!uninstfinalize',result)
            for unsafe in (text+'!system "x"\n',text+'!define OUTFILE "extra"\n',text.replace('COMMAND ""','COMMAND "run"')):
                with self.assertRaises(ValueError): transform_script(unsafe,Path(tmp)/'synthetic.exe')

if __name__=='__main__': unittest.main()
