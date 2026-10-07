# Palim 内部接入试验（2026-10-07）

## 后续决定：0.3.0 默认关闭精确数字特性

下面保留的是 0.2.2 本地草稿的原始试验快照。经审阅，默认开启仍会让普通依赖声明
改变下游数字行为，因此最终改为发布 0.3.0，`exact-numbers` 默认关闭。
需要 0.2 数字能力的调用方显式开启；Helixflow 改用普通声明 `palim = "0.3"`。
RFC `test` 与 compare 的 `1`/`1.0` 数学相等规则在两种配置下均保留。
本节的发布状态会在完成提交、远端 CI、上传与正式 registry 验证后更新。

0.3.0 本地验证：默认普通配置 debug/release 各 186 项及 1 doctest；显式
`exact-numbers` debug/release 各 203 项及 1 doctest。两种配置的全目标 Clippy、
Rust 1.85 library check、巨指数 benchmark 入口和 publish dry-run 均通过。
CI 同时覆盖普通默认和显式精确配置；独立 fuzz 工作区明确开启 `exact-numbers`。

## 原始 0.2.2 试验快照

结论：Helixflow 的手动 proposal 差异展示已有本地实现并通过完整工作区测试。
这是**尚未发布依赖版本的内部接入草稿**，不是已确认的外部用户、公开下游或生产采用，
也不验证 Palim 大数组移动、性能或可逆历史的采用价值。

Palim 基线：`ca2c28ab78b28a4ec92c25e63ba499f78d6a3e7a`（0.2.1）。
Helixflow 基线：`0f4992a4928a7aadeca449c3c97a7d2e92ec0d78`，工作分支
`feat/palim-proposal-diff`。本文描述这两个基线之上的本地未提交变更。
[完整 Helixflow 草稿补丁](helixflow-proposal-diff.patch) 可供独立审阅。
补丁采用零上下文格式；应用原始快照时使用 `git apply --unidiff-zero`。

## 接入选择与实际行为

并行检查了现有项目的 JSON 修改、列表身份和历史记录边界。
remem 的 SQL provenance、keyspoor 的指纹集合、cc-switch 的 SQLite/原始配置备份、
quotabar 的最新查询缓存、ccstats 的分析序列没有直接对应的 JSON 可逆列表历史需求。
Helixflow 的图节点是按 ID 索引的 map，边没有稳定 ID；没有把 map 改成数组或添加 ID 协议。

选用现有 `GraphService::preview_proposal` 和工作区的 pending proposal 读取路径。
旧摘要只有净节点数量和边数量，等数量替换节点、修改参数和布局无法准确展示；
重载持久化 proposal 时摘要为空。

新实现将两个图转成 JSON，用 `palim::compare` 生成 `+ /path`、`- /path`、`~ /path`。
摘要仅包含转义的 JSON Pointer 路径，不包含字段值。已有 `diffSummary: string[]` UI
可直接显示，前端、存储结构、应用操作、版本提交和回滚没有改动。
重载时以 proposal 自己的 immutable base version 比较；当前版本已前进时读取历史基线，
沿用现有路径/哈希校验并传播错误，不能读取有效基线时不生成假摘要。

Helixflow v2 的主 Agent 编辑路径是自动应用。保留的手动 proposal API/UI 是本次接入点；
不能把它描述为主 Agent 流程已经采用可逆 delta 或历史快照压缩。

## 确认的依赖语义问题与修复

原工作区 serde_json 1.0.151 没有启用 `arbitrary_precision` 或 `float_roundtrip`。
已发布的 Palim 0.2.1 无条件启用两者，Cargo 会统一工作区的依赖特性。
隔离程序使用 Helixflow SetParam 相同的 `Some(&current) != Some(&expected)` 表达式，
完成四种配置的实际运行：

| 配置 | 0.1 / 0.10 冲突 | 1.0 / 1e0 冲突 | 51.248178375505404 的 f64 bits |
|---|---|---|---|
| 原 serde_json | false | false | `40499fc44f1b2f61` |
| 仅 float_roundtrip | false | false | `40499fc44f1b2f60` |
| 仅 arbitrary_precision | true | true | `40499fc44f1b2f61` |
| Palim 0.2.1 | true | true | `40499fc44f1b2f60` |

这是已复现的语义变化。浮点行为更精确并不使它成为本次展示修改可以顺带改变的合同。
没有在 Helixflow 添加数字规范化 shim，也没有修改现有冲突判断。

Palim 0.2.2 草稿新增默认开启的 `exact-numbers` Cargo feature，保留原默认行为；
关闭默认特性时使用 serde_json 原来的数字能力。数字文本的内部辅助函数按启用配置
借用或拥有文本，保持默认任意精度路径；普通模式遵守 serde_json 的精度/序列化限制。
默认模式的原有严格 wire equality 断言保留，普通模式仍验证原始内存值的 patch/unpatch，
并针对同一解析器解码的文档验证 wire replay。只对真正依赖任意精度的测试或用例加 feature 条件。

Helixflow 声明 `palim = { version = "0.2.2", default-features = false }`。
打包后的 0.2.2 又完成四种外部 serde_json feature 配置运行；无额外特性时两个冲突均为 false，
浮点 bits 与基线相同。其他依赖主动启用特性时，Palim 的普通配置也正确处理数学相等和大数字回放。
`cargo tree` 确认接入后仍没有启用上述两个数字特性。

## 完成的检查

| 检查 | 实际结果 |
|---|---|
| Palim debug 默认 / 普通配置 | 203 + 1 doctest / 186 + 1 doctest 通过 |
| Palim release 默认 / 普通配置 | 203 + 1 doctest / 186 + 1 doctest 通过 |
| Palim 两种配置 all-targets Clippy，`-D warnings` | 通过 |
| Palim 两种配置 Rust 1.85 lib check | 通过 |
| Palim fmt、diff check | 通过 |
| Palim publish dry-run | 打包 48 文件并编译验证通过，未上传 |
| 打包后外部特性组合的隔离运行 | 四种组合通过 |
| Helixflow fmt、locked workspace check、diff check | 本地 Cargo patch 下通过 |
| Helixflow locked workspace tests | 574 通过，1 项原有测试 ignored，0 失败 |
| Helixflow strict workspace Clippy | 失败；干净基线复现相同 registry 告警 |
| Helixflow graph/server scoped strict Clippy | 失败于未修改的 server 文件告警 |

Helixflow 新测试覆盖：等数量节点替换、参数/布局路径、pointer 转义、数值相等、无修改、
深度错误传播、真实 SetParam 数字合同、实际存储重开、过期 proposal 的历史基线、
损坏历史基线和外工作区基线。

Clippy 基线问题是 `registry/src/catalog_seed.rs:387` 的 `too_many_arguments` 和
`registry/src/builtin.rs:48` 的 `filter_map_bool_then`。
scope 检查另外遇到未修改的 `server/src/layout_routes.rs:247` 的 unused lifetime 和
`server/src/ops_routes.rs:156` 的 collapsible if；没有顺带清理这些文件。

本地验证采用命令行覆盖，未写入机器专用 `.cargo/config.toml`：

```sh
# 在 Helixflow 仓库执行；路径对应本次工作区。
cargo check --workspace --locked --offline \
  --config 'patch.crates-io.palim.path="/Users/apple/Desktop/code/AI/tool/secrets/palim"'
cargo test --workspace --locked --offline \
  --config 'patch.crates-io.palim.path="/Users/apple/Desktop/code/AI/tool/secrets/palim"'

# 在 Palim 仓库执行。
cargo test --locked
cargo test --no-default-features --locked
cargo test --release --locked
cargo test --release --no-default-features --locked
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --no-default-features --locked -- -D warnings
cargo +1.85.0 check --lib --locked
cargo +1.85.0 check --lib --no-default-features --locked
cargo publish --dry-run --allow-dirty --locked
```

## 交付状态

0.2.2 尚未上传 crates.io，Helixflow lock 中当前 Palim 项是本地 patch 解析的条目。
因此 Helixflow 草稿还不能脱离覆盖配置当作正式 registry 依赖构建。
发布并可从 registry 取得 0.2.2 后，需要重新解析正常 source/checksum lock，
再执行不带本地覆盖的 locked 工作区 check/test。远端 CI 尚未运行。

本次没有发布、推送、修改用户运行数据库、联系外部项目或新增线上采用主张。
所有新增验证数据来自临时测试存储和合成 JSON。
