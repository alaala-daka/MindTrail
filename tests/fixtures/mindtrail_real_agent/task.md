# 合成工作

在当前空目录创建 `index.mjs`，导出 `double(value)`，返回输入的两倍；创建 `cli.mjs`，用 ESM 导入并调用该函数，输出 JSON 数字。用 `node cli.mjs 4` 和 `node cli.mjs -3` 验证输出分别为 `8` 和 `-6`。

使用本次会话及给定操作 ID 依次报告任务开始、能力意图；完成编码和验证后，报告 API、文件、从文件到 API 的 calls 边、能力完成、任务完成。完成事实包含 quote `命令入口调用 double` 和 context `已用正负输入验证输出`。明确错误时停止追加依赖事件；不得捏造完成。
