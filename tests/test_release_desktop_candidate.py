import json
import hashlib
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'tests')); sys.path.insert(0,str(ROOT/'scripts'))
import release_desktop_candidate as candidate

class DesktopCandidateTests(unittest.TestCase):
    def test_build_failure_quarantines_before_manifest(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            root=Path(tmp); run_id='a'*32; run=root/'.tools/productization-release'/('run-'+run_id); (run/'source/scripts').mkdir(parents=True)
            desktop={name:{'path':str(root/(name+'.exe')),'sha256':'a'*64} for name in ('node','csc','tauri_js','tauri_native')}
            names=('cl.exe','link.exe','rc.exe','mt.exe','vcvars64.bat'); producer={'tools':{name:{'path':str(root/name),'sha256':'a'*64} for name in names}}
            (run/'desktop-toolchain-inspection.json').write_text(json.dumps(desktop)); (run/'producer-toolchain-inspection-v2.json').write_text(json.dumps(producer))
            (run/'producer-build.json').write_text(json.dumps({'sha256':'b'*64})); (run/'nsis-toolset.sha256.json').write_text('{}')
            (run/'producer-resource.json').write_text('{}'); (run/'source-inputs.json').write_text('{}')
            dist=run/'source/dist'; dist.mkdir(); (dist/'index.html').write_text('synthetic'); dist_pin=candidate.fingerprint_tree(dist); (run/'frontend-dist.sha256').write_text(dist_pin)
            pin_names=('producer-toolchain-inspection-v2.json','desktop-toolchain-inspection.json','producer-build.json','nsis-toolset.sha256.json','producer-resource.json','source-inputs.json')
            pins={name:hashlib.sha256((run/name).read_bytes()).hexdigest() for name in pin_names}
            calls=[]
            def execute(argv,cwd,env,timeout):
                calls.append(argv)
                if len(calls)<=2:
                    dest=next(str(x)[5:] for x in argv if str(x).startswith('/out:')); Path(dest).write_bytes(b'synthetic')
            with mock.patch.object(candidate,'ROOT',root),mock.patch.object(candidate,'APPROVED_RUN',run_id),mock.patch.object(candidate,'APPROVED_DIST',dist_pin),mock.patch.object(candidate,'RECORD_PINS',pins),mock.patch.object(candidate,'desktop_preflight',return_value=0),mock.patch.object(candidate,'approved_pwsh',return_value=root/'pwsh.exe'),mock.patch.object(candidate,'validate_tool',return_value=True),mock.patch.object(candidate,'build_environment',return_value={'PATH':'synthetic'}),mock.patch.object(candidate,'capture_job',return_value=(0,'PATH=synthetic\nINCLUDE=synthetic\nLIB=synthetic\n','')),mock.patch.object(candidate,'capture_desktop_job',return_value={'exit_code':37,'timed_out':False}),mock.patch.object(candidate.shutil,'which',return_value=str(root/'tool.exe')),mock.patch.object(candidate,'run_command',side_effect=execute):
                with self.assertRaisesRegex(ValueError,'desktop_build_failed'): candidate.main(run_id)
            self.assertEqual(len(calls),2); self.assertTrue((run/'.release-quarantine').exists()); self.assertFalse((run/'release/manifest.json').exists()); self.assertEqual(json.loads((run/'desktop-diagnostic.json').read_text())['exit_code'],37)
    def test_cargo_hardlinked_desktop_is_validated_before_release_copy(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            root=Path(tmp); release=root/'release'; deps=release/'deps'; deps.mkdir(parents=True)
            pe=bytearray(512); pe[:2]=b'MZ'; pe[60:64]=(128).to_bytes(4,'little'); pe[128:132]=b'PE\0\0'; pe[132:134]=(0x8664).to_bytes(2,'little')
            exe=release/'mindtrail.exe'; exe.write_bytes(pe)
            try: os.link(exe,deps/'mindtrail.exe')
            except OSError: self.skipTest('hardlinks unavailable')
            self.assertEqual(candidate.validate_built_pe(exe),hashlib.sha256(bytes(pe)).hexdigest())
            self.assertEqual(candidate.validate_pe(exe),hashlib.sha256(bytes(pe)).hexdigest())
            copy=root/'copied.exe'; copy.write_bytes(pe)
            self.assertEqual(candidate.validate_pe(copy),hashlib.sha256(bytes(pe)).hexdigest())
            (deps/'mindtrail.exe').unlink(); peer=release/'other.exe'; os.link(exe,peer)
            with self.assertRaisesRegex(ValueError,'desktop_invalid'): candidate.validate_built_pe(exe)
            with self.assertRaisesRegex(ValueError,'producer_invalid'): candidate.validate_pe(exe)
    def test_dist_change_is_detected(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            root=Path(tmp); (root/'index.html').write_text('first'); before=candidate.fingerprint_tree(root)
            (root/'index.html').write_text('second'); self.assertNotEqual(candidate.fingerprint_tree(root),before)
    def test_nsis_requires_exact_install_and_uninstall_entries(self):
        with tempfile.TemporaryDirectory(dir=ROOT/'.tools') as tmp:
            root=Path(tmp); source=root/'producer.exe'; desktop=root/'MindTrail.exe'
            source.write_bytes(b'synthetic'); desktop.write_bytes(b'synthetic')
            valid=('!define MAINBINARYNAME "mindtrail"\n!define MAINBINARYSRCPATH "'+str(desktop)+'"\n'
                   '!define PRODUCTNAME "MindTrail"\n!define INSTALLMODE "currentUser"\n'
                   'Section Install\nStrCpy $INSTDIR "$LOCALAPPDATA\\${PRODUCTNAME}"\n'
                   'File "${MAINBINARYSRCPATH}"\nCreateDirectory "$INSTDIR\\producer"\n'
                   'WriteUninstaller "$INSTDIR\\uninstall.exe"\nSetOutPath $INSTDIR\n'
                   'File /a "/oname=producer\\mindtrail-producer.exe" "'+str(source)+'"\nSectionEnd\n'
                   'Section Uninstall\nDelete "$INSTDIR\\${MAINBINARYNAME}.exe"\n'
                   'Delete "$INSTDIR\\producer\\mindtrail-producer.exe"\nDelete "$INSTDIR\\uninstall.exe"\n'
                   'RMDir /REBOOTOK "$INSTDIR\\producer"\nSectionEnd\n'
                   'CreateShortcut "$SMPROGRAMS\\${PRODUCTNAME}.lnk" "$INSTDIR\\${MAINBINARYNAME}.exe"\n'
                   'CreateShortcut "$DESKTOP\\${PRODUCTNAME}.lnk" "$INSTDIR\\${MAINBINARYNAME}.exe"\n')
            with mock.patch.object(candidate,'validate_pe',return_value='a'*64),mock.patch.object(candidate,'validate_built_pe',return_value='b'*64):
                self.assertTrue(candidate.validate_nsis_script(valid,[source],'a'*64,desktop))
                variants=(valid.replace('/oname=producer\\','/oname=elsewhere\\'),valid.replace('Delete "$INSTDIR\\producer','; Delete "$INSTDIR\\producer'),valid.replace(str(source),str(source.parent/'other.exe')),valid.replace('SetOutPath $INSTDIR','SetOutPath "$TEMP"'),valid.replace('Section Uninstall','Section Other'))
                for text in variants:
                    with self.subTest(text=text),self.assertRaisesRegex(ValueError,'package_static_failed'): candidate.validate_nsis_script(text,[source],'a'*64,desktop)

if __name__=='__main__': unittest.main()
