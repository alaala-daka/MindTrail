// Candidate PATH guard: caller must bind tool, directories, ownership and clean environment.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;

internal static class ReleaseCargoPreflight {
    private static string Absolute(string value) {
        if (String.IsNullOrEmpty(value) || value.IndexOf('"')>=0 || !Path.IsPathRooted(value) || value.StartsWith("\\\\",StringComparison.Ordinal)) throw new ArgumentException();
        string full=Path.GetFullPath(value);
        if (!String.Equals(value.TrimEnd('\\'),full.TrimEnd('\\'),StringComparison.OrdinalIgnoreCase)) throw new ArgumentException();
        return full;
    }
    private static string Quote(string value) { if(value.IndexOf('"')>=0) throw new ArgumentException(); return "\""+value+"\""; }
    private static int Main(string[] args) {
        try {
            string cargo=Absolute(Environment.GetEnvironmentVariable("MINDTRAIL_CARGO_EXE"));
            string source=Absolute(Environment.GetEnvironmentVariable("MINDTRAIL_SOURCE_ROOT"));
            string cwd=Absolute(Environment.GetEnvironmentVariable("MINDTRAIL_CARGO_CWD"));
            if (args.Length!=4 || args[0]!="metadata" || !((args[1]=="--format-version" && args[2]=="1" && args[3]=="--no-deps") || (args[1]=="--no-deps" && args[2]=="--format-version" && args[3]=="1"))) { Console.Error.WriteLine("cargo_preflight_rejected"); return 2; }
            string manifest=Path.Combine(source,"src-tauri","Cargo.toml");
            var arguments=new List<string>{"metadata","--format-version","1","--no-deps","--manifest-path",Quote(manifest),"--locked","--offline"};
            var info=new ProcessStartInfo { FileName=cargo, Arguments=String.Join(" ",arguments.ToArray()),WorkingDirectory=cwd,UseShellExecute=false };
            using(var process=Process.Start(info)) { process.WaitForExit(); return process.ExitCode; }
        } catch { Console.Error.WriteLine("cargo_preflight_failed"); return 1; }
    }
}
