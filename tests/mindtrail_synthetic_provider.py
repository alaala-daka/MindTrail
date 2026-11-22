"""Fixed synthetic Responses SSE generator; never logs request content."""
import json


def synthetic_response(request, session, operation_id, first, *, command=False, command_shape='write', call_index=1):
    if not isinstance(request, dict) or not isinstance(request.get('tools'), list):
        raise ValueError('invalid_synthetic_request')
    matches = [child for item in request['tools'] if isinstance(item, dict)
               and item.get('type') == 'namespace' and item.get('name') == 'mcp__mindtrail'
               and isinstance(item.get('tools'), list) for child in item['tools']
               if isinstance(child, dict) and child.get('name') == 'trace_task_start']
    if first and command and sum(isinstance(item, dict) and item.get('name') == 'exec_command' for item in request['tools']) != 1:
        raise ValueError('synthetic_tool_missing')
    if first and not command and len(matches) != 1:
        raise ValueError('synthetic_tool_missing')
    if first:
        if command:
            if command_shape not in ('write', 'read-only', 'write-execute'):
                raise ValueError('invalid_command_shape')
            if command_shape == 'write-execute':
                if call_index not in (1, 2):
                    raise ValueError('invalid_command_shape')
                fixed = "Set-Content -LiteralPath synthetic-command.js -Value 'console.log(42)' -Encoding ascii" if call_index == 1 else 'node synthetic-command.js'
            else:
                fixed = 'echo synthetic > synthetic-command.txt' if command_shape == 'write' else 'echo synthetic'
            item = {'id': 'fc_synthetic' if call_index == 1 else 'fc_synthetic_2', 'type': 'function_call', 'status': 'completed',
                    'call_id': 'call_synthetic' if call_index == 1 else 'call_synthetic_2', 'name': 'exec_command',
                    'arguments': json.dumps({'cmd': fixed})}
        else:
            item = {'id': 'fc_synthetic', 'type': 'function_call', 'status': 'completed',
                    'call_id': 'call_synthetic', 'namespace': 'mcp__mindtrail', 'name': 'trace_task_start',
                    'arguments': json.dumps({'session': session, 'operation_id': operation_id,
                                             'title': 'synthetic', 'goal': 'synthetic', 'project': 'synthetic'})}
    else:
        item = {'id': 'msg_synthetic', 'type': 'message', 'status': 'completed', 'role': 'assistant',
                'content': [{'type': 'output_text', 'text': 'done', 'annotations': []}]}
    response = {'id': 'resp_synthetic', 'object': 'response', 'created_at': 0,
                'status': 'completed', 'model': 'synthetic-model', 'output': [item],
                'usage': {'input_tokens': 1, 'output_tokens': 1, 'total_tokens': 2}}
    events = [
        {'type': 'response.created', 'response': {**response, 'status': 'in_progress', 'output': []}},
    ]
    if first:
        events.extend([
            {'type': 'response.output_item.added', 'output_index': 0,
             'item': {**item, 'status': 'in_progress', 'arguments': ''}},
            {'type': 'response.function_call_arguments.delta', 'item_id': item['id'],
             'output_index': 0, 'delta': item['arguments']},
            {'type': 'response.function_call_arguments.done', 'item_id': item['id'],
             'output_index': 0, 'arguments': item['arguments']},
        ])
    else:
        events.append({'type': 'response.output_item.added', 'output_index': 0, 'item': item})
    events.extend([{'type': 'response.output_item.done', 'output_index': 0, 'item': item},
                   {'type': 'response.completed', 'response': response}])
    return b''.join(('event: ' + e['type'] + '\ndata: ' + json.dumps(e) + '\n\n').encode('utf-8') for e in events) + b'data: [DONE]\n\n'
