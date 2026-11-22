"""Release configuration must not use the user's Tauri tool cache."""
import json
from pathlib import Path
import unittest

ROOT=Path(__file__).resolve().parent.parent
class ReleaseConfigTests(unittest.TestCase):
    def test_local_bundle_tools_and_webview_skip(self):
        config=json.loads((ROOT/'src-tauri/tauri.conf.json').read_text(encoding='utf8'))
        bundle=config['bundle']
        self.assertIs(bundle['useLocalToolsDir'],True)
        self.assertEqual(bundle['windows']['webviewInstallMode']['type'],'skip')
        self.assertEqual(bundle['resources'],{'resources/producer/mindtrail-producer.exe':'producer/mindtrail-producer.exe'})

if __name__=='__main__': unittest.main()
