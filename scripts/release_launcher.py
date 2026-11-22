"""Bounded Windows-job primitive, not a production build entry point."""
import subprocess
from pathlib import Path
from release_job import OwnedJob
from release_environment import build_environment
from release_tools import validate_tool

def run_job(argv,*,cwd,env,timeout_seconds,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL):
    if not 0 < timeout_seconds <= 3600: raise ValueError('invalid_timeout')
    job=OwnedJob(argv,cwd=cwd,env=env,stdout=stdout,stderr=stderr)
    try:
        try: return job.process.wait(timeout=timeout_seconds)
        except subprocess.TimeoutExpired: raise TimeoutError('release_child_timeout') from None
    finally: job.close()

def run_pinned_job(argv,*,expected_executable,expected_sha256,run,cargo_cwd,system_root,tool_directories,rustc,timeout_seconds):
    """Run one caller-authorized command; ownership and argv policy remain caller obligations."""
    if type(argv) is not list or not argv: raise ValueError('invalid_command')
    validate_tool(argv[0],expected_executable,expected_sha256)
    env=build_environment(run,cargo_cwd,system_root,tool_directories,rustc)
    try:
        return run_job(argv,cwd=cargo_cwd,env=env,timeout_seconds=timeout_seconds)
    finally:
        validate_tool(argv[0],expected_executable,expected_sha256)

def run_isolated_python(python,script,arguments,**kwargs):
    """Always suppress user site, PYTHONPATH and script-directory imports."""
    if not Path(script).is_absolute() or type(arguments) is not list: raise ValueError('invalid_python_command')
    return run_pinned_job([str(python),'-I',str(script),*arguments],**kwargs)
