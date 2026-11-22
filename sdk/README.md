# MindTrail 生产者接入

Windows 生产者通过本机网关提交事件。请使用构建好的 `mindtrail-producer.exe` 的**绝对路径**，并为每个独立生产者分配自己的队列。下面的路径是示例，请替换为本机路径；不要把网关 token 写进配置或命令行。

## MCP

Windows Codex CLI 的配置和事件顺序见 [Codex 接入说明](../docs/mindtrail-codex.md)。

在支持 MCP stdio 的客户端中配置命令和参数数组：

```json
{
  "mcpServers": {
    "mindtrail": {
      "command": "C:\\Apps\\MindTrail\\mindtrail-producer.exe",
      "args": ["mcp", "--queue", "my_agent"]
    }
  }
}
```

每个工具调用必须给出 `session` 和稳定的 `operation_id`，可选 `quote`、`context`。调用方生成唯一 ID；同一操作重试必须在原队列使用相同 ID 和相同参数。不同生产者使用不同队列和全局唯一的操作 ID。

一个会话可按以下顺序记录事实（括号内列出各工具的专用字段）：

1. `trace_task_start(title, goal, project)`。
2. `trace_intent(id, title, summary?)`，例如能力 ID `cap-search`。
3. `trace_api(id, signature, kind, capability?, summary?)`，例如 API ID `api-search`，`capability` 引用 `cap-search`；`kind` 为 `http`、`cli` 或 `fn`。
4. `trace_file(id, path, action, capability?, purpose?, add?, del?)`，例如文件 ID `file-search`，`action` 为 `create`、`modify` 或 `delete`。
5. `trace_call(from, to, kind?)`，例如从 `api-search` 到 `file-search`；`kind` 为 `calls` 或 `imports`，默认 `calls`。
6. `trace_done(id, summary?)`，`id` 引用前面的能力 ID。
7. `trace_task_done(summary?)`，结束会话。

文件统计缺省为零。客户端断开不会自动结束任务。`queued` 表示事件已保存在本地队列；`committed` 表示网关已确认并返回 seq。MCP 进程存活时会自动恢复待发操作；重启后也会继续。跨生产者存在先后依赖时，等待前一操作 `committed` 后再发送依赖它的事实。

## TypeScript / Node

仓库源码为 `sdk/typescript/index.ts`。使用本项目依赖构建可供 Node 使用的 JavaScript 和类型声明：

```powershell
npx tsc --ignoreConfig --target ES2020 --module NodeNext --moduleResolution NodeNext --types node --skipLibCheck --strict --declaration --outDir dist/mindtrail-sdk sdk/typescript/index.ts
```

在 ESM 项目中导入编译后的模块，并明确提供生产者程序路径：

```ts
import { MindTrailProducer, type EventV1 } from './dist/mindtrail-sdk/index.js';

const producer = new MindTrailProducer('C:\\Apps\\MindTrail\\mindtrail-producer.exe');
const events: EventV1[] = [{
  v: 1, session: 'example-node-session', seq: 1,
  ts: '2024-01-01T00:00:00Z', type: 'task.start',
  payload: { title: 'Example', goal: 'Record work', project: 'demo' },
}];
const first = await producer.submitEvents('node_agent', 'stable-batch-id', events);
const retry = await producer.submitEvents('node_agent', 'stable-batch-id', events);
const counts = await producer.flush('node_agent');
```

## Python

将 `sdk/python` 放入模块搜索路径：

```python
from mindtrail_producer import MindTrailProducer

producer = MindTrailProducer(r"C:\Apps\MindTrail\mindtrail-producer.exe")
events = [{"v": 1, "session": "example-python-session", "seq": 1,
           "ts": "2024-01-01T00:00:00Z", "type": "task.start",
           "payload": {"title": "Example", "goal": "Record work", "project": "demo"}}]
first = producer.submit_events("python_agent", "stable-batch-id", events)
retry = producer.submit_events("python_agent", "stable-batch-id", events)
counts = producer.flush("python_agent")
```

显式批次由调用方分配 session、连续正 seq 和时间戳；包装器不会排序或修改事件，也不会自动重试。调用返回 `queued`、`committed` 或 `{error: {code, message, ...}}`；`flush` 返回 committed、pending、blocked 数量或错误。错误中可包含协议的数值详情。`producer_failed` 表示无法可靠解释子进程结果，可用相同队列、批次 ID 和事件重新调用。显式 CLI 进程结束后不会在后台继续发送；再次调用 `submitEvents` / `submit_events` 或 `flush` 才会触发恢复。包装器限制输出和进程时长，默认 10 秒，可按构造器选项调整。

## 手动 curl 重试

以下脚本读取调用者指定的受保护发现文件和完整 UTF-8 批次文件，并通过 curl stdin 配置传递认证头；不要把 token 加入命令行。脚本校验健康返回的库身份、HTTP 状态、事件数、回执和高水位，使用固定 IPv4 回环、禁用代理及重定向并设定时限。发现文件最多 4 KiB，响应最多 64 KiB；脚本要求 curl 8.4 或更高版本，以便在响应长度未知时也中止超限下载。它只进行一次手动发送；再次执行相同文件可重试。

```powershell
& .\sdk\examples\submit-events.ps1 `
  -DiscoveryPath 'C:\Users\me\.path-tracker\endpoint.json' `
  -BatchPath 'C:\Work\events.json' `
  -ScratchDirectory 'C:\Work'
```

`events.json` 是完整 v1 数组，所有事件属于同一 session，按 seq 严格递增。输出仅是确认回执；调用者负责检查并保留它。脚本不承担缺口修复或后台续传。
