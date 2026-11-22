from pathlib import Path
import sys
import unittest
from unittest import mock

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'tests')); sys.path.insert(0,str(ROOT/'scripts'))
import release_desktop_preflight as desktop

class DesktopPreflightTests(unittest.TestCase):
    def test_only_fixed_pwsh_path_and_hash_accepted(self):
        program=r'C:\Program Files'
        approved=Path(program)/'WindowsApps'/desktop.PWSH_PACKAGE/'dependencies/native/powershell/pwsh.exe'
        record={'tools':{'pwsh.exe':{'path':str(approved),'sha256':desktop.PWSH_SHA256}}}
        with mock.patch('release_tools.validate_tool',return_value=True) as validate:
            self.assertEqual(desktop.approved_pwsh(program,record),approved)
            validate.assert_called_once_with(approved,approved,desktop.PWSH_SHA256,allow_hardlinks=True)
            for path,digest in ((str(approved.parent/'other.exe'),desktop.PWSH_SHA256),(str(approved),'0'*64)):
                bad={'tools':{'pwsh.exe':{'path':path,'sha256':digest}}}
                with self.assertRaisesRegex(ValueError,'pwsh_pin_mismatch'): desktop.approved_pwsh(program,bad)
            self.assertEqual(validate.call_count,1)

if __name__=='__main__': unittest.main()
