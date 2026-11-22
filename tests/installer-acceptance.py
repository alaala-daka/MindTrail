"""Installer acceptance runner; never trusts an environment name alone.

Default behaviour records a blocked result and performs no install. Installation
steps require a separate isolation-evidence file and an explicit --execute.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent

STEPS = (
    'snapshot_recorded', 'install', 'installed_files', 'webview2_preinstalled',
    'launch', 'mcp_seven_tools', 'offline_queue_recovery', 'real_client',
    'upgrade', 'upgrade_data_stable', 'uninstall', 'uninstall_data_retained',
    'reinstall_restore', 'registry_shortcuts',
)
ISOLATION_FIELDS = {
    'environment_id', 'hypervisor', 'snapshot_id', 'snapshot_sha256',
    'restore_tested', 'network_blocked', 'preinstalled_webview2', 'windows_11_x64',
}
EVIDENCE_KEYS = {
    'path', 'size', 'sha256', 'version', 'old_version', 'new_version', 'count',
    'retained', 'tools', 'registry', 'shortcuts', 'note', 'producer', 'desktop', 'store',
}
STATUSES = {'passed', 'failed', 'not_run', 'blocked'}
HEX64 = re.compile(r'^[a-f0-9]{64}$')
STEP_NAME = re.compile(r'^[a-z0-9_]{3,40}$')


class ReportError(ValueError):
    pass


def _scalar(value):
    return value is None or type(value) in (str, int, bool)


def _evidence(value, depth=0):
    if depth > 3 or _scalar(value):
        return
    if isinstance(value, dict):
        if any(not isinstance(key, str) or key not in EVIDENCE_KEYS for key in value):
            raise ReportError('invalid_evidence')
        for item in value.values():
            _evidence(item, depth + 1)
        return
    if isinstance(value, list) and len(value) <= 16:
        for item in value:
            _evidence(item, depth + 1)
        return
    raise ReportError('invalid_evidence')


def validate_report(report, *, manifest_installer=None):
    if not isinstance(report, dict) or set(report) != {'schema', 'kind', 'environment', 'installer', 'steps', 'result'}:
        raise ReportError('invalid_report')
    if report['schema'] != 1 or report['kind'] != 'installer_acceptance':
        raise ReportError('invalid_report')
    installer = report['installer']
    if not isinstance(installer, dict) or set(installer) != {'path', 'size', 'sha256'}:
        raise ReportError('invalid_installer')
    if (not isinstance(installer['path'], str) or Path(installer['path']).name != installer['path']
            or type(installer['size']) is not int or installer['size'] < 1
            or not isinstance(installer['sha256'], str) or not HEX64.fullmatch(installer['sha256'])):
        raise ReportError('invalid_installer')
    if manifest_installer is not None and (
            not isinstance(manifest_installer, dict)
            or installer['path'] != Path(str(manifest_installer.get('path', ''))).name
            or installer['size'] != manifest_installer.get('size')
            or installer['sha256'] != manifest_installer.get('sha256')):
        raise ReportError('installer_mismatch')
    environment = report['environment']
    if not isinstance(environment, dict) or set(environment) != {'id', 'isolation'}:
        raise ReportError('invalid_environment')
    if not isinstance(environment['id'], str) or not re.fullmatch(r'[A-Za-z0-9._-]{1,80}', environment['id']):
        raise ReportError('invalid_environment')
    isolation = environment['isolation']
    if isolation is not None:
        if not isinstance(isolation, dict) or set(isolation) != ISOLATION_FIELDS:
            raise ReportError('invalid_isolation')
        if (not isinstance(isolation['environment_id'], str) or isolation['environment_id'] != environment['id']
                or not isinstance(isolation['hypervisor'], str) or not isolation['hypervisor']
                or not isinstance(isolation['snapshot_id'], str) or not isolation['snapshot_id']
                or not isinstance(isolation['snapshot_sha256'], str) or not HEX64.fullmatch(isolation['snapshot_sha256'])
                or any(type(isolation[name]) is not bool for name in
                       ('restore_tested', 'network_blocked', 'preinstalled_webview2', 'windows_11_x64'))):
            raise ReportError('invalid_isolation')
    steps = report['steps']
    if not isinstance(steps, list) or len(steps) != len(STEPS):
        raise ReportError('invalid_steps')
    seen = []
    for step in steps:
        if not isinstance(step, dict) or set(step) != {'name', 'status', 'evidence', 'limitation'}:
            raise ReportError('invalid_step')
        if (not isinstance(step['name'], str) or not STEP_NAME.fullmatch(step['name'])
                or step['name'] not in STEPS or step['name'] in seen):
            raise ReportError('invalid_step')
        seen.append(step['name'])
        if step['status'] not in STATUSES:
            raise ReportError('invalid_step')
        if not isinstance(step['limitation'], str) or len(step['limitation']) > 400:
            raise ReportError('invalid_step')
        _evidence(step['evidence'])
    if set(seen) != set(STEPS):
        raise ReportError('missing_step')
    if report['result'] not in ('passed', 'failed', 'blocked'):
        raise ReportError('invalid_result')
    by_name = {step['name']: step for step in steps}
    passed = all(step['status'] == 'passed' for step in steps)
    if report['result'] == 'passed':
        if isolation is None or by_name['snapshot_recorded']['status'] != 'passed':
            raise ReportError('missing_isolation')
        installed = by_name['installed_files']['evidence'] or {}
        producer = installed.get('producer') if isinstance(installed, dict) else None
        if not isinstance(producer, dict) or not isinstance(producer.get('sha256'), str) or not HEX64.fullmatch(producer['sha256']):
            raise ReportError('missing_installed_producer')
        upgrade = by_name['upgrade']['evidence'] or {}
        if (not isinstance(upgrade, dict) or not isinstance(upgrade.get('old_version'), str)
                or not isinstance(upgrade.get('new_version'), str) or upgrade['old_version'] == upgrade['new_version']):
            raise ReportError('upgrade_version_unchanged')
        retained = by_name['uninstall_data_retained']['evidence'] or {}
        if not isinstance(retained, dict) or retained.get('retained') is not True:
            raise ReportError('uninstall_retention_unproven')
        if any(step['status'] != 'passed' for step in steps):
            raise ReportError('incomplete_steps')
    elif passed:
        raise ReportError('incomplete_steps')
    return True


def load_manifest(manifest_path):
    path = Path(manifest_path)
    if not path.is_file():
        raise ReportError('missing_manifest')
    try:
        with path.open('rb') as stream:
            if os.fstat(stream.fileno()).st_size > 1048576:
                raise ReportError('invalid_manifest')
            data = stream.read(1048577)
        manifest = json.loads(data, object_pairs_hook=_unique)
    except (OSError, ValueError):
        raise ReportError('invalid_manifest') from None
    if not isinstance(manifest, dict) or manifest.get('signing') != 'unsigned_candidate':
        raise ReportError('invalid_manifest')
    files = manifest.get('files')
    installer = None
    if isinstance(files, list):
        for item in files:
            if isinstance(item, dict) and item.get('path') in ('installer/MindTrail-setup.exe', 'installer/mindtrail-setup.exe'):
                installer = item
                break
    if installer is None:
        raise ReportError('missing_installer_record')
    return manifest, {'path': installer['path'], 'size': installer['size'], 'sha256': installer['sha256']}


def _unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('duplicate')
        result[key] = value
    return result


def file_fingerprint(path):
    path = Path(path)
    info = path.lstat()
    if path.is_symlink() or not path.is_file() or os.path.getsize(path) > 536870912:
        raise ReportError('invalid_installer')
    digest = hashlib.sha256()
    size = 0
    with path.open('rb') as stream:
        before = os.fstat(stream.fileno())
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            size += len(block)
            digest.update(block)
        after = os.fstat(stream.fileno())
    if (before.st_dev, before.st_ino, before.st_size) != (after.st_dev, after.st_ino, after.st_size) or size != after.st_size:
        raise ReportError('installer_changed')
    return {'path': path.name, 'size': size, 'sha256': digest.hexdigest()}


def write_blocked(report_dir, installer, environment_id, reason, manifest_installer=None):
    steps = [
        {'name': name, 'status': 'blocked' if name == 'snapshot_recorded' else 'not_run',
         'evidence': None, 'limitation': reason}
        for name in STEPS
    ]
    report = {
        'schema': 1, 'kind': 'installer_acceptance',
        'environment': {'id': environment_id, 'isolation': None},
        'installer': installer, 'steps': steps, 'result': 'blocked',
    }
    validate_report(report, manifest_installer=manifest_installer)
    report_dir.mkdir(parents=True, exist_ok=False)
    (report_dir / 'result.json').write_text(json.dumps(report, indent=2, sort_keys=True), encoding='utf8')
    return report


def read_isolation(path, environment_id):
    path = Path(path)
    try:
        with path.open('rb') as stream:
            if os.fstat(stream.fileno()).st_size > 65536:
                return None, 'isolation_evidence_too_large'
            data = json.loads(stream.read(65537), object_pairs_hook=_unique)
    except (OSError, ValueError):
        return None, 'isolation_evidence_missing'
    if not isinstance(data, dict) or set(data) != ISOLATION_FIELDS:
        return None, 'isolation_evidence_invalid'
    if (data.get('environment_id') != environment_id or not data.get('restore_tested')
            or not data.get('network_blocked') or not data.get('preinstalled_webview2')
            or not data.get('windows_11_x64')):
        return None, 'isolation_evidence_unproven'
    return data, None


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument('--installer', required=True)
    parser.add_argument('--release-manifest', required=True)
    parser.add_argument('--report', required=True)
    parser.add_argument('--isolated-environment-id', required=True)
    parser.add_argument('--isolation-evidence')
    parser.add_argument('--execute', action='store_true')
    args = parser.parse_args(argv)
    report_dir = Path(args.report)
    if report_dir.exists():
        print('installer_acceptance=refused_existing_report')
        return 1
    try:
        _, manifest_installer = load_manifest(args.release_manifest)
        installer = file_fingerprint(args.installer)
        if installer['sha256'] != manifest_installer['sha256'] or installer['size'] != manifest_installer['size']:
            raise ReportError('installer_mismatch')
    except ReportError as error:
        print('installer_acceptance=' + str(error))
        return 1
    if not args.execute or not args.isolation_evidence:
        write_blocked(report_dir, installer, args.isolated_environment_id,
                      'blocked_missing_dependency: isolated_windows_environment', manifest_installer)
        print('installer_acceptance=blocked_missing_dependency')
        return 2
    evidence, reason = read_isolation(args.isolation_evidence, args.isolated_environment_id)
    if evidence is None:
        write_blocked(report_dir, installer, args.isolated_environment_id, 'blocked_missing_dependency: ' + reason, manifest_installer)
        print('installer_acceptance=blocked_missing_dependency')
        return 2
    # Execution from here requires a disposable VM and is deliberately not
    # automated from the daily workstation; record the verified gate instead.
    write_blocked(report_dir, installer, args.isolated_environment_id,
                  'blocked_missing_dependency: automated_install_driver_not_available', manifest_installer)
    print('installer_acceptance=blocked_missing_dependency')
    return 2


if __name__ == '__main__':
    sys.exit(main())
