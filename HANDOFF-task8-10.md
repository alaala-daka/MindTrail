# MindTrail Task 8–10 交接

记录日期：2026-11-21。面向接手本工作区的 Agent。目标是完成产品化的打包发布、隔离安装验收和真实 AI 解读验收。以当前文件、进程状态和实际测试结果为准；本记录不是完成证明。

> **后续状态（2026-11-21，以本段为准）**：Task 8 已生成并校验候选 run `02bb4e8bbebd45da906552912bc6cb2a`，Task 9 无凭据真实桌面验收 14/14 通过，Task 10 默认/缺凭据路径已用最终 harness 与测试重生成（`semantic-run4`、`semantic-live-blocked-run4`，均晚于 harness/测试修改；run3 及更早为历史）。另按用户要求用 Qoder DeepSeek-Flash（`dfmodel`）做只读复核，修复了成功门、硬编码预览范围（现与投递事件/原话数量交叉核对）、凭据泄露到 Vite、引用核对和假接线测试等问题；验收 unittest 运行 39 项、38 项通过、1 项跳过，全量 Python 运行 177 项、175 项通过、2 项跳过；审计中另修复 installer 校验器与 manifest 相对路径比对缺陷，并以 `installer-run2` 重新生成 blocked 安装报告。DeepSeek-Flash 复核确认该修复 CLOSED、无新可达 fail-open（仅保留不传 manifest 时跳过比对的低危宽松）。DeepSeek-Flash 不是计划要求的 Sol reviewer，其执行也不是真实 provider 请求。安装仍因缺少可恢复隔离环境为 `blocked_missing_dependency`，真实 provider 仍为 `blocked_missing_credentials`，整体阶段未完成。详见 `docs/superpowers/execution/mindtrail-productization/{acceptance,review}.md` 的 2026-11-21 记录；下方旧 run 状态仅作历史。

## 先看这里

**Task 8 尚未完成：没有经过验证的正式 Windows 安装包、发布清单或最终审查。** Task 9、10 尚未完成。不要把模拟结果、测试安装器或旧版本包当成本次发布结果。

工作区为 `C:\Users\aefar\source\repos\MindTrail`，Windows/PowerShell。存在大量未提交和未跟踪修改及权限测试现场；`.git` 当前只读。不要清理未知目录、修改既有 ACL、覆盖他人修改或执行 `git add .`。`docs/` 被现有规则忽略；重要交付文档不要误以为会自动被 Git 跟踪。执行前检查当前权限和 `git status`；HEAD 单独不能代表当前脏工作树。

用户已选择**自行配置 OpenAI 兼容 API**。不读取认证目录、密钥或用户日常客户端配置；不在聊天中要求粘贴密钥。此前 Agent 接入验收使用独立认证目录和真实模型，但这不授权将其认证用于 AI provider 验收。

之前用户要求各阶段由 6.1 Sol 研究规格、6.1 Sol high 写计划、Terra high 实施、6.1 Sol 独立审查；适用时延续该流程。阶段通过后再进入下一阶段。不要将用量限制当作账号余额结论，也不要擅自更换用户指定模型。

## 现状与证据

| 范围 | 现状 | 证据及限制 |
| --- | --- | --- |
| 原生 producer | 已成功构建并独立复核（历史 run 现已隔离） | run `073c61ec455a4e66a77ce92756332df7`，release/x64、locked/offline、`producer` feature；4,769,280 字节，SHA-256 `918d21cf3fcd2bc752709a30f9f25c9c360aaa5a48e51802c6f2810453dd5591`。见 `task8-producer-config-build.log` 和 `review.md`。该 run 现已被隔离，最终候选使用 run `02bb4e8b…` 的 producer（SHA-256 `240b2bb2…b16ee`），不是此历史产物。它不是安装验证或运行期验证。 |
| 前端和依赖 | 隔离前端构建、Cargo/npm 离线准备已通过 | 相关日志见下方索引。不同 run 和源码版本不能混用；构建前须重新核对源码、资源及工具摘要。 |
| NSIS 工具 | 官方固定版本归档及插件已准备，解包文件清单曾独立核对 | 清单是解包后生成的完整性记录；不单独证明供应链来源。工具完整和摘要一致不代表安装通过。 |
| 桌面打包 | 先前两次桌面尝试失败；生成了桌面 exe 和 NSIS 脚本，但未生成正式安装器/清单 | `task8-desktop-build.log`、`task8-desktop-retry-build.log`；失败 run 保留隔离标记，不要解除或用于发布。 |
| NSIS 诊断 | 受限环境中的 `makensis -NOCONFIG -VERSION` 返回 Windows 错误 623；普通权限版本检查成功，隔离诊断夹具编译成功 | `task8-nsis-version-normal.log`、`task8-nsis-diagnostic-normal.log`。这支持进程环境差异是合理线索，但不证明先前 Tauri 失败的唯一根因。测试安装器未运行，也不是正式安装包。 |
| 下一候选 run | `aab09ef6900a45eb9056d02f541f163f` 已准备 producer/资源、离线 npm、前端和桌面 preflight；该 run 现已隔离，最终入口固定 run `02bb4e8bbebd45da906552912bc6cb2a` | `tests/release_desktop_candidate.py` 的 `APPROVED_RUN` 现为 `02bb…`。旧 `aab09e…` 的失败现场保留，不得复用；最终候选的六份记录、dist、工具 pin、NSIS 已在 2026-11-21 复核。 |
| 真实 Agent | 事件接入验收通过 | `docs/superpowers/execution/mindtrail-real-agent/acceptance.md`。自动恢复使用守门观察器，keepalive 后被取消，不等同原始 stdio 自然完成；不证明真实 AI 语义生成或安装。 |

日志目录：`.tools/productization-task3-a4a3eadf70a04c1a93a14f113e360079/`。各独占构建目录在 `.tools/productization-release/run-<id>/`。日志有些只包含固定 `passed/failed`，不要推断未保存的原始命令输出。旧记录可能按时间追加而非倒序，结合 `review.md`、实际文件和时间阅读。

## Task 8：打包发布，优先完成

完整要求以 `docs/superpowers/plans/2026-11-18-mindtrail-productization.md` 的 Task 8 和 `docs/superpowers/execution/mindtrail-productization/task8-plan.md`、`task8-spec.md` 为准。首验范围是 Windows 11 x64、预装 WebView2、NSIS、未签名候选；不是公开发布或已验证安装。

1. 只读核对最终 run `02bb4e8bbebd45da906552912bc6cb2a`、当前源码与脚本差异、owner/quarantine、producer 和资源摘要、封存 dist、工具和 NSIS 清单及 `APPROVED_*`。旧 `aab09ef…` 已隔离，仅作失败历史，不得复用。确认没有旧桌面 exe、安装器和 NSIS 脚本；若任何输入变化，重新暂存并审查，不清除失败 run 的隔离标记。注意某些候选脚本位于 `tests/`，要核对实际构建输入及其版本绑定。
2. 查阅最新审查，安排独立 reviewer 对**具体新 run 和当前入口**做构建前复核。受控普通权限已被用于只读版本探针和隔离 NSIS 测试；实际构建若需要权限升级，遵守当前环境审批，不能绕过自动审批。只生成本地候选，不安装、不更改客户端或系统防火墙。Cargo/npm 使用已准备的离线依赖，NSIS 使用本轮已核验工具；未实施 OS 级网络阻断，不能宣称整个打包过程绝不联网。
3. 按审查通过的入口执行一次桌面/NSIS 构建，记录命令、退出码和失败类别。失败立即停止、保留并隔离该轮，定位实际错误后修复，不凭空重复尝试；不要压掉足以定位问题的诊断。若产物生成，验证新鲜来源、桌面和 producer x64 PE、producer 与资源摘要相同、NSIS 脚本安装目标及卸载条目、安装器及大小/SHA-256。静态检查不等同运行或安装。
4. 完成可重复的发布编排、`manifest.json`、`scripts/verify-release.ps1` 校验和 `docs/release.md`。清单按主计划记录版本、目标、脏源码摘要、文件路径/大小/SHA-256、`unsigned_candidate` 与各检查状态；安装、升级、卸载、真实客户端和真实 provider 未运行时必须标记未验证。核验生成后源码与包摘要，运行适当回归并安排最终独立审查。给出候选路径和摘要后才可说 Task 8 完成。

## Task 9：真实桌面与隔离安装验收

依据主计划 Task 9 和产品化设计执行。先完善无凭据真实桌面 runner 与测试，覆盖空态、连接帮助、合成事件/图/回放、事实、模拟解读与引用、取消/切换/离线错误。浏览器 mock 或本机自检不替代真实桌面和真实客户端。

安装、升级、卸载必须在有明确快照/恢复证据的隔离 Windows 11 x64 环境进行；仅提供环境名称不构成隔离证明。验证已安装桌面与 producer 的实际路径和摘要、预装 WebView2、MCP 七工具、离线队列与恢复、真实客户端接入。验证不同版本升级后数据及配置稳定、卸载默认保留 `.path-tracker` 中事实和队列、重装恢复，并记录注册表及快捷方式变化。若隔离 VM/可恢复环境或客户端不可用，记录具体 `blocked_missing_dependency`，不得在日常用户环境绕过限制，也不要宣称安装通过。保留硬断电和目录项持久性未验证的限制。

交付计划所列 `tests/productization-desktop.py`、`tests/installer-acceptance.py`、`docs/quick-start.md`、`docs/troubleshooting.md` 及验收报告；先确认是否已有实现，再补齐。Task 9 的安装结论独立于 Task 8 静态包检查。

## Task 10：真实 AI 解读与最终审查

依据主计划 Task 10 实现并验证 `tests/semantic-live.py` 和验收负例。默认不联网；真实验收必须使用真实 Rust provider 和桌面命令链、隔离的最小合成事实，展示发送范围后一次生成。仅在用户明确配置的环境中读取所需变量，不读取认证文件或记录密钥、完整 URL、提示及原始响应；单次请求、30 秒、无自动重试，401/429 等按真实类别记录。缺少明确配置时记 `blocked_missing_credentials`，继续完成不依赖凭据的工作，不以 mock 充当真实服务结果。

核对结果引用属于同一前缀事实，生成前后事件数量/摘要及任务状态不变。完成最终前端、Rust、Python、浏览器和真实桌面回归，重新计算源码/包摘要；最终 Sol 独立审查完整 diff、manifest 和证据。报告应区分代码完成、无凭据验证、真实 provider 和安装验证；缺证据就如实保留未完成状态。

## 文件索引

| 文件或目录 | 用途 |
| --- | --- |
| `docs/superpowers/specs/2026-11-18-mindtrail-productization-design.md` | 产品目标、用户流程和边界；是早期规格，部分代码现状描述已过时。 |
| `docs/superpowers/plans/2026-11-18-mindtrail-productization.md` | Task 1–10 的主计划及验收条件。 |
| `docs/superpowers/execution/mindtrail-productization/{task8-spec.md,task8-plan.md,acceptance.md,review.md,dependencies.md}` | Task 8 专项规格、步骤、执行证据和独立审查；检查文件实际最新内容。 |
| `docs/superpowers/execution/mindtrail-real-agent/{acceptance.md,review.md}` | 真实 Agent 验收结果及观察器限制。 |
| `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `src-tauri/build.rs` | 桌面配置、producer feature、构建入口。 |
| `src-tauri/src/{connection.rs,distribution.rs,gateway/,producer/,semantic/}` | 连接自检、producer 资源定位、事实存储与网关、MCP 和 AI 语义服务。 |
| `src/workspace/{ConnectionPanel.tsx,connection.ts,InsightDrawer.tsx,semantic.ts}`, `src/App.tsx` | 接入帮助、配置复制、语义预览与工作台入口。 |
| `scripts/release_producer_build.py`, `scripts/prepare-producer.ps1`, `scripts/prepare-release-stage.ps1`, `scripts/release-source*.ps1` | producer 构建、暂存与源码一致性。注意检查 fail-closed 状态和实际入口。 |
| `scripts/release_{environment,launcher,job,tools,dependencies,nsis_readiness,nsis_extract}.py`, `scripts/release-cargo-{preflight,wrapper}.cs` | 受控环境、进程、工具和 NSIS 预检/提取、Tauri Cargo 守门。 |
| `scripts/verify-release.ps1`, `scripts/release-build-hook.ps1` | 发布清单验证和构建 hook；需核对当前可用性，不能假设已完成。 |
| `tests/release_desktop_candidate.py`, `tests/release_desktop_preflight.py`, `tests/release_desktop_diagnostics.py` | 当前固定 run 的桌面候选、预检和受限诊断。 |
| `tests/release_{resource_prepare,transfer_producer}.ps1`, `tests/release_nsis_diagnostic.py` | producer 资源准备、迁移及隔离 NSIS 定向诊断；测试夹具不是正式包。 |
| `tests/test_productization_acceptance.py`, `tests/test_release_*.py`, `tests/productization-browser.py` | 发布相关负例及浏览器验证；具体覆盖范围以测试源码和日志为准。 |
| `protocol/`, `sdk/`, `docs/mindtrail-codex.md` | 事件协议、生产者 SDK 和 MCP 接入说明。 |

## 环境与操作注意

- 可用的本地 Rust 工具目录为 `.tools/cargo`、`.tools/rustup`；隔离运行时应按已审脚本固定环境，不继承用户凭据或任意配置。源码目录直接 Cargo 扫描曾被旧 ACL 测试夹具阻断；不要删除或修改该现场。核验源码的独占副本构建是不同证据，报告时应说明。
- 工具版本、哈希、run 路径和固定脚本常量可能因后续修改而过期。先核验，再复用；不要直接信任本文的历史通过状态。
- 不读取或泄露真实凭据，不自动修改日常 Codex 配置，不自动发布公网、不使用签名私钥。安装器安装和真实 provider 属各自验收阶段；未完成前不得将 Task 8 的静态检查写成安装成功。
- 若使用本地资源包，先核对来源、摘要和路径；不要从未知历史 fixture 或 PATH 取旧产物顶替新构建。不要假设符号链接测试通过：部分用例曾因 Windows 权限跳过。
