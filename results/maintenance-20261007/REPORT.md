# Palim 维护复核（2026-10-07）

源码基线：`0d1a041fa09859d51a344ec89ec7b2f6e180c223`，版本 0.3.0。
本轮只修改基准文档与复核证据，没有修改生产源码、API、错误合同或已有性能数字。
原有 worktree 和 results 保留；文档改动位于独立分支。

## 反馈与采用证据

GitHub 的全部 issue/PR 列表只有维护者 `majiayu000` 的
[已合并 PR #1](https://github.com/majiayu000/palim/pull/1)，没有 issue 或开放 PR。
没有取得外部采用者的新输入、性能或 API 反馈，因此不从其它库的 issue 推断 Palim
外部用户需要新能力，也不编造外部试用。公开页面/API 的范围只代表公开反馈。

README 记录的 [Helixflow 内部集成](../internal-integration-20261007/REPORT.md)
及 [已合并 PR #225](https://github.com/majiayu000/helixflow/pull/225) 保留为内部采用证据，
不算外部采用或生产效果。本轮未重跑该集成。

## 文档修正

0.3.0 默认不启用 serde_json 的 `arbitrary_precision` 和 `float_roundtrip`；
历史 0.2 及更早测量启用了这些特性。BENCHMARK.md 现明确区分版本与数字合同，
并在复现命令中显式启用 `exact-numbers`。普通解析可能在 Palim 接收 Value 前已舍入
或拒绝超范围数字；Cargo 特性统一也可能由其它依赖启用精确数字。

所有历史测量值、旧版本状态和证据原样保留。这次复核没有正式计时、排名或新的
速度 claim；RFC dry-run 只验证入口与输出，不是重新测量整套 benchmark。

## 远端验证

[当前源码的 CI](https://github.com/majiayu000/palim/actions/runs/37594965417)
为 completed/success，绑定 `0d1a041`；包含 Linux、Windows、macOS Rust 矩阵、
Rust 1.85、JavaScript 互通、60 秒 fuzz。Rust 矩阵的默认/exact-number debug/release
命令以该提交的 ci.yml 为准。这是已存在的源码 CI，不是本次文档 PR 的 CI。
本机没有再次运行跨 OS、release 全套、Clippy、Criterion 或 fuzz。

## 本机重新完成的检查

检查明细、命令、结果及完整日志见 [verification.json](verification.json)。
重新检查的输入来自原生 suite、现有 1,073 组冻结/固定种子互通输入，以及现有 RFC
fixture；不作为真实业务输入或新增外部用户效果。检查过程使用独立输出目录，
未覆盖旧互通 summary、verification.json 或历史测量。

- 默认数字配置：187 项 debug/unit/integration/doctest；exact-number 配置：204 项，全部通过。
- Rust 1.85 的两种数字配置库检查通过。
- 1,073 组必需 JS 互通路径全部通过；562 个格式合法、JS 自逆成功的 inverse 由默认 fuzzy 还原，4 个长度头错误仍拒绝。507 个 JS 自逆失败另列，不作为 Palim 必需路径失败。
- RFC pipeline/export/apply/inverse 入口共 7 个引擎/fixture dry-run 验证通过，计时样本数为 0；只覆盖小配置与 2,000 项 replace fixture，没有重跑完整压力矩阵。

## 公开用户输入重放的限制与来源

复用 [既有调查](../github-user-needs-20261003/REPORT.md) 与
[冻结输入来源标签](../github-user-needs-20261003/fixtures.json)：原始公开 issue 数据、
JSON 转换及代表性合成案例分别保留标签。它们是其它库用户公开提交的场景，
不表示这些用户采用 Palim。原复现程序的消费者直接启用 serde_json 精确数字特性，
因此不能把该消费者结果当作 0.3 普通数字默认配置验收。

原程序首次未完成：全新 standalone release 编译超过原有 180 秒限额。
临时副本复用本仓库 target 后完成编译，但后续独立 JS 大数组进程超过原有 30 秒限额。
两个失败日志保留；没有放宽产品行为、断言、内存限制或修改原冻结程序。
最后一轮只在临时副本中复用构建缓存、将进程限时从 30 秒设为 90 秒，并保留单进程
1 GiB V8 old-space 边界。它与原复现条件有差异，不能称原脚本未经修改通过。

最终临时副本重放退出码为 0。29 个 Palim 场景及 17 个 JS 正向 delta 导入场景完成；
与旧结果对照的 187 个明确功能/拒绝结果全部一致，没有 probe error。
错误基线、通配符和外部错误 inverse 的既有拒绝/限制继续保留，未把它们误算作成功修复。
2.2 万项字符串数组的前向、撤销、plain/optimized RFC 及独立 plain inverse 成功，
仍生成 34 个标准操作。临时程序保留诊断计时，但本轮有明显系统负载波动，
不据此发布新倍率、排名或生产性能承诺。
