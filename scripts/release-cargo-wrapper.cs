// Candidate only: its caller must validate ownership, tools and the complete environment.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;

internal static class ReleaseCargoWrapper {
    private static string Absolute(string value) {
        if (String.IsNullOrEmpty(value) || value.IndexOf('"')>=0 || !Path.IsPathRooted(value) || value.StartsWith("\\\\",StringComparison.Ordinal)) throw new ArgumentException();
        string full=Path.GetFullPath(value);
        if (!String.Equals(value.TrimEnd('\\'),full.TrimEnd('\\'),StringComparison.OrdinalIgnoreCase)) throw new ArgumentException();
        return full;
    }
    private static string Quote(string value) { if (value.IndexOf('"')>=0) throw new ArgumentException(); return "\""+value+"\""; }
    private static int Main(string[] args) {
        try {
            string cargo=Absolute(Environment.GetEnvironmentVariable("MINDTRAIL_CARGO_EXE"));
            string source=Absolute(Environment.GetEnvironmentVariable("MINDTRAIL_SOURCE_ROOT"));
            string cwd=Absolute(Environment.GetEnvironmentVariable("MINDTRAIL_CARGO_CWD"));
            string manifest=Path.Combine(source,"src-tauri","Cargo.toml");
            if (args.Length==0 || args[0]!="build") { Console.Error.WriteLine("cargo_wrapper_command_rejected"); return 2; }
            var forwarded=new List<string>{"build","--manifest-path",Quote(manifest),"--locked","--offline"};
            bool release=false,target=false,bins=false,features=false;
            for(int i=1;i<args.Length;i++) {
                if(args[i]=="--release" && !release) { release=true; forwarded.Add("--release"); }
                else if(args[i]=="--bins" && !bins) { bins=true; forwarded.Add("--bins"); }
                else if(args[i]=="--features" && !features && i+1<args.Length && args[i+1]=="tauri/custom-protocol") { features=true; forwarded.Add("--features"); forwarded.Add(args[++i]); }
                else if(args[i]=="--target" && !target && i+1<args.Length && args[i+1]=="x86_64-pc-windows-msvc") { target=true; forwarded.Add("--target"); forwarded.Add(args[++i]); }
                else {
                    string tag=args[i]=="--features" && i+1<args.Length && args[i+1]=="tauri/custom-protocol" ? "features_custom_protocol" : args[i]=="--features" ? "features_other" : args[i]=="--bins" ? "bins" : args[i]=="--lib" ? "lib" : args[i]=="--bin" ? "bin" : args[i]=="--message-format=json" ? "message_format" : "other";
                    Console.Error.WriteLine("cargo_wrapper_argument_rejected_"+tag); return 2;
                }
            }
            if (!release || !target) { Console.Error.WriteLine("cargo_wrapper_required_flags_missing"); return 2; }
            var info=new ProcessStartInfo { FileName=cargo,Arguments=String.Join(" ",forwarded.ToArray()),WorkingDirectory=cwd,UseShellExecute=false };
            using(var process=Process.Start(info)) { process.WaitForExit(); return process.ExitCode; }
        } catch { Console.Error.WriteLine("cargo_wrapper_failed"); return 1; }
    }
}
