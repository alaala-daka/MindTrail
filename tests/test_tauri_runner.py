"""Run the installed Tauri CLI only against a synthetic project and fake runner."""
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest

ROOT=Path(__file__).resolve().parent.parent
CLI=ROOT/'node_modules/@tauri-apps/cli/tauri.js'
TOOLCHAIN=ROOT/'.tools/rustup/toolchains/stable-x86_64-pc-windows-msvc/bin'
NODE=Path(os.environ['ProgramFiles'])/'nodejs/node.exe'
sys.path.insert(0,str(ROOT/'scripts'))
from release_launcher import run_job

class TauriRunnerTests(unittest.TestCase):
    def test_synthetic_runner_cwd_and_argv(self): self._probe(False)
    def test_synthetic_wrapper_binds_manifest(self): self._probe(True)
    def test_tauri_cargo_preflight_observation(self): self._probe(False,True)
    def test_tauri_guarded_preflight_binds_manifest(self): self._probe(False,'guard')
    def test_tauri_guarded_preflight_continues_to_runner(self): self._probe(False,'continued')
    def test_tauri_synthetic_build_without_bundle(self): self._probe(False,'success')
    def test_combined_guards_propagate_build_failure(self): self._probe(True,'combined-fail')
    def test_combined_guards_synthetic_success(self): self._probe(True,'combined-success')
    def _probe(self,use_wrapper,intercept=False):
        windows=Path(os.environ['SystemRoot']); temp=windows/'Temp'
        for d in (temp,windows,Path(windows.anchor)):
            for p in ('.cargo/config','.cargo/config.toml','.npmrc'): self.assertFalse((d/p).exists(),'unsafe_ancestor_config')
        with tempfile.TemporaryDirectory(prefix='mindtrail-tauri-runner-',dir=temp) as raw:
            base=Path(raw); self.assertEqual(base.resolve().parent,temp.resolve()); self.assertTrue(base.name.startswith('mindtrail-tauri-runner-'))
            project=base/'project'; native=project/'src-tauri'; controlled=base/'controlled'; home=base/'home'
            for d in (native/'src',project/'dist',controlled,home): d.mkdir(parents=True)
            fake=base/'runner.exe'; fake_source=base/'runner.cs'
            fake_source.write_text('''using System; using System.IO;
class Runner { static void Main(string[] a) {
bool cwd=String.Equals(Directory.GetCurrentDirectory(),Environment.GetEnvironmentVariable("PROBE_EXPECT_CWD"),StringComparison.OrdinalIgnoreCase);
bool meta=Array.IndexOf(a,"metadata")>=0; int mi=Array.IndexOf(a,"--manifest-path"); bool manifest=mi>=0 && mi+1<a.Length && String.Equals(a[mi+1],Environment.GetEnvironmentVariable("PROBE_EXPECT_MANIFEST"),StringComparison.OrdinalIgnoreCase);
string data="{\\"cwd_matches\\":"+cwd.ToString().ToLowerInvariant()+",\\"has_build\\":"+(Array.IndexOf(a,"build")>=0).ToString().ToLowerInvariant()+",\\"has_marker\\":"+(Array.IndexOf(a,"--synthetic-runner-marker")>=0).ToString().ToLowerInvariant()+",\\"first_build\\":"+(a.Length>0 && a[0]=="build").ToString().ToLowerInvariant()+",\\"has_config_arg\\":"+(Array.IndexOf(a,"--synthetic-config-arg")>=0).ToString().ToLowerInvariant()+",\\"has_manifest\\":"+(mi>=0).ToString().ToLowerInvariant()+",\\"manifest_matches\\":"+manifest.ToString().ToLowerInvariant()+",\\"has_locked\\":"+(Array.IndexOf(a,"--locked")>=0).ToString().ToLowerInvariant()+",\\"has_offline\\":"+(Array.IndexOf(a,"--offline")>=0).ToString().ToLowerInvariant()+",\\"has_release\\":"+(Array.IndexOf(a,"--release")>=0).ToString().ToLowerInvariant()+",\\"has_target\\":"+(Array.IndexOf(a,"--target")>=0).ToString().ToLowerInvariant()+"}";
string response=Environment.GetEnvironmentVariable("PROBE_METADATA_RESPONSE");
string sequence=Environment.GetEnvironmentVariable("PROBE_SEQUENCE");
bool safe=cwd && manifest && Array.IndexOf(a,"--locked")>=0 && Array.IndexOf(a,"--offline")>=0;
if(meta) safe=safe && a.Length==8 && Array.IndexOf(a,"--no-deps")>=0 && Array.IndexOf(a,"--format-version")>=0;
else safe=safe && Array.IndexOf(a,"--release")>=0 && Array.IndexOf(a,"--target")>=0;
if(sequence!=null) File.AppendAllText(sequence,(meta ? "metadata_" : "build_")+(safe ? "safe" : "invalid")+"\\n");
File.WriteAllText(Environment.GetEnvironmentVariable(meta && response!=null ? "PROBE_PREFLIGHT_RESULT" : "PROBE_RESULT"),data);
if(meta && response!=null) { Console.Write(File.ReadAllText(response)); Environment.Exit(0); }
if(!meta && Environment.GetEnvironmentVariable("PROBE_BUILD_OK")=="1") { string p=Environment.GetEnvironmentVariable("PROBE_FAKE_EXE"); Directory.CreateDirectory(Path.GetDirectoryName(p)); File.WriteAllText(p,"synthetic"); Environment.Exit(0); } Environment.Exit(19); } }''',encoding='utf8')
            compiler=windows/'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
            csc_env={'SystemRoot':str(windows),'WINDIR':str(windows),'TEMP':str(base),'TMP':str(base),'PATH':str(windows/'System32')}
            compiled=subprocess.run([str(compiler),'/noconfig','/nologo','/out:'+str(fake),str(fake_source)],cwd=controlled,env=csc_env,capture_output=True,timeout=30)
            errors=','.join(re.findall(r'CS\d{4}',(compiled.stdout+compiled.stderr).decode('utf8','replace')))
            self.assertEqual(compiled.returncode,0,'synthetic_runner_compile_failed:'+errors)
            wrapper=base/'wrapper.exe'
            if use_wrapper:
                built=subprocess.run([str(compiler),'/noconfig','/nologo','/reference:System.dll','/out:'+str(wrapper),str(ROOT/'scripts/release-cargo-wrapper.cs')],cwd=controlled,env=csc_env,capture_output=True,timeout=30)
                self.assertEqual(built.returncode,0,'wrapper_compile_failed')
            if intercept:
                fakebin=base/'fakebin'; fakebin.mkdir(); fake_cargo=fakebin/'cargo.exe'; fake_cargo_source=base/'fakecargo.cs'
                fake_cargo_source.write_text('''using System; using System.IO; class Probe { static void Main(string[] a) {
bool cwd=String.Equals(Directory.GetCurrentDirectory(),Environment.GetEnvironmentVariable("PROBE_EXPECT_NATIVE"),StringComparison.OrdinalIgnoreCase);
int unknown=0; foreach(string v in a) { if(v!="metadata" && v!="--format-version" && v!="1" && v!="--no-deps") unknown++; }
string data="{\\"cwd_native\\":"+cwd.ToString().ToLowerInvariant()+",\\"metadata\\":"+(Array.IndexOf(a,"metadata")>=0).ToString().ToLowerInvariant()+",\\"version\\":"+(Array.IndexOf(a,"--version")>=0 || Array.IndexOf(a,"-V")>=0).ToString().ToLowerInvariant()+",\\"manifest\\":"+(Array.IndexOf(a,"--manifest-path")>=0).ToString().ToLowerInvariant()+",\\"locked\\":"+(Array.IndexOf(a,"--locked")>=0).ToString().ToLowerInvariant()+",\\"offline\\":"+(Array.IndexOf(a,"--offline")>=0).ToString().ToLowerInvariant()+",\\"format\\":"+(Array.IndexOf(a,"--format-version")>=0).ToString().ToLowerInvariant()+",\\"no_deps\\":"+(Array.IndexOf(a,"--no-deps")>=0).ToString().ToLowerInvariant()+",\\"argc\\":"+a.Length.ToString()+",\\"unknown\\":"+unknown.ToString()+"}";
File.WriteAllText(Environment.GetEnvironmentVariable("PROBE_CARGO_SIGNAL"),data); Environment.Exit(19); } }''',encoding='ascii')
                guard_source=ROOT/'scripts/release-cargo-preflight.cs' if intercept in ('guard','continued','success','combined-fail','combined-success') else fake_cargo_source
                refs=['/reference:System.dll'] if intercept in ('guard','continued','success','combined-fail','combined-success') else []
                built=subprocess.run([str(compiler),'/noconfig','/nologo',*refs,'/out:'+str(fake_cargo),str(guard_source)],cwd=controlled,env=csc_env,capture_output=True,timeout=30)
                self.assertEqual(built.returncode,0,'fake_cargo_compile_failed')
            (native/'Cargo.toml').write_text('[package]\nname="synthetic_probe"\nversion="0.1.0"\nedition="2021"\n',encoding='utf8')
            (native/'src/main.rs').write_text('fn main() {}',encoding='utf8')
            (project/'package.json').write_text('{"name":"synthetic-probe","version":"0.1.0"}',encoding='utf8')
            (project/'dist/index.html').write_text('<html></html>',encoding='utf8')
            config={'productName':'Synthetic','version':'0.1.0','identifier':'org.example.synthetic',
                    'build':{'frontendDist':'../dist','runner':{'cmd':str(wrapper if use_wrapper else fake),'cwd':str(controlled),'args':[] if use_wrapper else ['--synthetic-config-arg']}},
                    'app':{'windows':[]},'bundle':{'active':False}}
            (native/'tauri.conf.json').write_text(json.dumps(config),encoding='utf8')
            result=base/'result.json'
            env={'SystemRoot':str(windows),'WINDIR':str(windows),'PATH':str(TOOLCHAIN)+os.pathsep+str(windows/'System32'),
                 'TEMP':str(base),'TMP':str(base),'USERPROFILE':str(home),'HOME':str(home),'CARGO_HOME':str(home),
                 'CARGO_NET_OFFLINE':'true','PROBE_EXPECT_CWD':str(controlled),'PROBE_RESULT':str(result),'CI':'1'}
            env.update({'PROBE_EXPECT_MANIFEST':str(native/'Cargo.toml'),'MINDTRAIL_CARGO_EXE':str(fake),'MINDTRAIL_SOURCE_ROOT':str(project),'MINDTRAIL_CARGO_CWD':str(controlled)})
            if intercept:
                env['PATH']=str(fakebin)+os.pathsep+env['PATH']; env['PROBE_CARGO_SIGNAL']=str(base/'cargo-called'); env['PROBE_EXPECT_NATIVE']=str(native)
            if intercept in ('continued','success','combined-fail','combined-success'):
                response=base/'metadata.json'
                with response.open('wb') as metadata:
                    code=run_job([str(TOOLCHAIN/'cargo.exe'),'metadata','--format-version','1','--no-deps','--offline','--manifest-path',str(native/'Cargo.toml')],cwd=controlled,env=env,timeout_seconds=30,stdout=metadata)
                self.assertEqual(code,0,'synthetic_metadata_failed')
                env['PROBE_METADATA_RESPONSE']=str(response); env['PROBE_PREFLIGHT_RESULT']=str(base/'preflight-result.json')
            if intercept in ('combined-fail','combined-success'): env['PROBE_SEQUENCE']=str(base/'sequence.txt')
            if intercept in ('success','combined-success'):
                env['PROBE_BUILD_OK']='1'; env['PROBE_FAKE_EXE']=str(native/('target/x86_64-pc-windows-msvc/release/synthetic_probe.exe' if use_wrapper else 'target/release/synthetic_probe.exe'))
            command=[str(NODE),str(CLI),'build','--no-bundle','--ci','--ignore-version-mismatches']
            if intercept in ('success','combined-success'): command+=['--no-binary-patching']
            if use_wrapper: command+=['--target','x86_64-pc-windows-msvc']
            if not use_wrapper: command+=['--','--synthetic-runner-marker']
            with tempfile.TemporaryFile(dir=base) as output, tempfile.TemporaryFile(dir=base) as errors:
                code=run_job(command,cwd=project,env=env,timeout_seconds=60,stdout=output,stderr=errors)
                output.seek(0); errors.seek(0)
                completed=types.SimpleNamespace(returncode=code,stdout=output.read(),stderr=errors.read())
            if intercept in ('combined-fail','combined-success'):
                self.assertTrue((base/'preflight-result.json').exists()); self.assertTrue(result.exists())
                self.assertEqual((base/'sequence.txt').read_text().splitlines(),['metadata_safe']*6+['build_safe'])
                self.assertTrue(json.loads((base/'preflight-result.json').read_text())['manifest_matches'])
                self.assertTrue(json.loads(result.read_text())['manifest_matches'])
                if intercept=='combined-success':
                    self.assertEqual(completed.returncode,0,'synthetic_combined_not_complete')
                    self.assertTrue(Path(env['PROBE_FAKE_EXE']).exists())
                else: self.assertNotEqual(completed.returncode,0,'synthetic_failure_not_propagated')
                print('fixed_combined='+json.dumps({'metadata_calls':6,'build_calls':1,'success':intercept=='combined-success'},sort_keys=True)); return
            if intercept=='success':
                self.assertTrue((base/'preflight-result.json').exists(),'guard_not_invoked')
                self.assertTrue(result.exists(),'synthetic_runner_not_invoked')
                self.assertTrue(Path(env['PROBE_FAKE_EXE']).exists(),'synthetic_exe_not_created')
                self.assertEqual(completed.returncode,0,'synthetic_build_not_complete')
                print('fixed_synthetic_build='+json.dumps({'completed':True,'bundle_requested':False},sort_keys=True)); return
            self.assertNotEqual(completed.returncode,0)
            if intercept=='continued':
                preflight=base/'preflight-result.json'; self.assertTrue(preflight.exists(),'guard_not_invoked')
                observed=json.loads(preflight.read_text()); self.assertTrue(observed['cwd_matches'] and observed['manifest_matches'] and observed['has_locked'] and observed['has_offline'])
                self.assertTrue(result.exists(),'runner_not_invoked_after_metadata')
                self.assertTrue(json.loads(result.read_text())['has_build'])
                print('fixed_continued='+json.dumps({'preflight_guarded':True,'runner_called':True},sort_keys=True)); return
            if intercept=='guard':
                self.assertTrue(result.exists(),'guard_target_not_invoked')
                observed=json.loads(result.read_text())
                self.assertEqual(observed,{'cwd_matches':True,'has_build':False,'has_marker':False,'first_build':False,'has_config_arg':False,'has_manifest':True,'manifest_matches':True,'has_locked':True,'has_offline':True,'has_release':False,'has_target':False})
                for args in (['metadata','--manifest-path','other'],['metadata','--config','x'],['build'],['metadata','--format-version','1','--no-deps','--config','x']):
                    result.unlink(missing_ok=True)
                    rejected=subprocess.run([str(fake_cargo),*args],cwd=native,env=env,capture_output=True,timeout=15)
                    self.assertNotEqual(rejected.returncode,19); self.assertFalse(result.exists())
                print('fixed_guarded_preflight='+json.dumps(observed,sort_keys=True)); return
            if intercept:
                observation={'cargo_called':(base/'cargo-called').exists(),'runner_called':result.exists()}
                self.assertEqual(observation,{'cargo_called':True,'runner_called':False})
                call=json.loads((base/'cargo-called').read_text())
                self.assertEqual(call,{'argc':4,'cwd_native':True,'format':True,'locked':False,'manifest':False,'metadata':True,'no_deps':True,'offline':False,'unknown':0,'version':False})
                print('fixed_preflight='+json.dumps(observation,sort_keys=True)); print('fixed_cargo_call='+json.dumps(call,sort_keys=True)); return
            if not result.exists():
                message=completed.stderr.lower()+completed.stdout.lower()
                categories=('cargo_wrapper_argument_rejected_features_custom_protocol','cargo_wrapper_argument_rejected_features_other','cargo_wrapper_argument_rejected_bins','cargo_wrapper_argument_rejected_lib','cargo_wrapper_argument_rejected_bin','cargo_wrapper_argument_rejected_message_format','cargo_wrapper_argument_rejected_other','cargo_wrapper_command_rejected','cargo_wrapper_failed','cargo','rustc','metadata','manifest','version','config','tauri','not found','no such file','failed','file','directory','os error','package','icon','webview','executable','git','node','not installed','platform')
                self.fail('runner_not_invoked:'+','.join(word for word in categories if word.encode() in message))
            observed=json.loads(result.read_text())
            self.assertEqual(observed,{'cwd_matches':True,'has_build':True,'has_marker':not use_wrapper,'first_build':True,'has_config_arg':not use_wrapper,'has_manifest':use_wrapper,'manifest_matches':use_wrapper,'has_locked':use_wrapper,'has_offline':use_wrapper,'has_release':True,'has_target':use_wrapper})
            print('fixed_observation='+json.dumps(observed,sort_keys=True))
            if use_wrapper:
                for args in (['build'],['build','--release'],['build','--target','x86_64-pc-windows-msvc'],['build','--manifest-path','other'],['build','--config','x'],['build','--target-dir','x'],['build','--target','other'],['build','--features','unapproved'],['build','--release','--release'],['build','@args']):
                    result.unlink(missing_ok=True)
                    rejected=subprocess.run([str(wrapper),*args],cwd=controlled,env=env,capture_output=True,timeout=15)
                    self.assertNotEqual(rejected.returncode,19,'override_reached_runner')
                    self.assertFalse(result.exists(),'override_reached_runner')
                for value in ('relative',r'\\server\share\source',str(project/'..'/'other')):
                    result.unlink(missing_ok=True); bad_env=env.copy(); bad_env['MINDTRAIL_SOURCE_ROOT']=value
                    rejected=subprocess.run([str(wrapper),'build'],cwd=controlled,env=bad_env,capture_output=True,timeout=15)
                    self.assertNotEqual(rejected.returncode,19,'invalid_root_reached_runner')
                    self.assertFalse(result.exists(),'invalid_root_reached_runner')

if __name__=='__main__': unittest.main()
