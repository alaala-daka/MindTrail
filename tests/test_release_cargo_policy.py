import importlib.util
from pathlib import Path
import unittest

ROOT=Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('release_cargo_policy',ROOT/'scripts/release_cargo_policy.py')
policy=importlib.util.module_from_spec(spec); spec.loader.exec_module(policy)

class CargoPolicyTests(unittest.TestCase):
    def test_only_fixed_manifest_and_flags(self):
        args=policy.cargo_arguments(r'C:\synthetic\source',['build','--release','--bins','--features','tauri/custom-protocol','--target',policy.TARGET])
        self.assertEqual(args,['build','--manifest-path',r'C:\synthetic\source\src-tauri\Cargo.toml','--locked','--offline','--release','--bins','--features','tauri/custom-protocol','--target',policy.TARGET])
    def test_overrides_and_unexpected_values_rejected(self):
        for args in (['build'],['build','--release'],['build','--target',policy.TARGET],['build','--manifest-path','other'],['build','--config','x'],['build','--target-dir','x'],
                     ['build','--target','other'],['build','--release','--release'],['build','@args'],['test'],['build','--features','x']):
            with self.subTest(args=args),self.assertRaises(ValueError): policy.cargo_arguments(r'C:\synthetic\source',args)
    def test_relative_unc_and_traversal_rejected(self):
        for path in ('relative',r'C:\synthetic\..\source',r'\\server\share\source','C:\\bad"path'):
            with self.subTest(path=path),self.assertRaises(ValueError): policy.cargo_arguments(path,['build'])

if __name__=='__main__': unittest.main()
