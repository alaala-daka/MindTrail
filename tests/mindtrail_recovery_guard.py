"""Read-only frame gate for the owned recovery MCP session."""
import json


def classify_recovery_frame(raw):
    """Permit only MCP startup/list; reject all calls and malformed frames."""
    def unique(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError('duplicate')
            result[key] = value
        return result
    if not isinstance(raw, bytes) or not raw.endswith(b'\n') or len(raw) > 1048576:
        return 'reject'
    try:
        obj = json.loads(raw, object_pairs_hook=unique)
    except (ValueError, UnicodeError, RecursionError):
        return 'reject'
    if not isinstance(obj, dict) or obj.get('jsonrpc') != '2.0':
        return 'reject'
    method = obj.get('method')
    if method in ('initialize', 'tools/list') and type(obj.get('id')) in (int, str):
        return 'startup'
    if method == 'notifications/initialized' and 'id' not in obj:
        return 'startup'
    return 'reject'
