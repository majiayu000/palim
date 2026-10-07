# 接收目标所有权的 RFC 新增/替换原型 — 2026-10-07

## 结论与范围

预拥有目标文档时，原型在宽子树新增的完整 pipeline 上有收益，**没有达到预估的 4–7 倍**。
最终三进程测量中，add-wide-100000 为 64.77 ms，现有 Palim 为 84.09 ms，json-patch
为 79.10 ms；分别减少约 23.0% 和 18.1%。标量修改相对现有 Palim 没有决定性优势。
如果调用方需要先 clone 目标，Core 收益基本消失。

本轮只做 scratchpad 原型，不新增库的公开 API、依赖或选项，不改变现有 plain 路径。
README 收窄到大数组重排与可逆变更；[ID 列表示例](../../examples/id_list_history.rs)
展示重排并编辑、插入、删除、序列化历史、逐步撤销/重做和 RFC 客户端回放。
这个示例不是外部采用证据；目前尚未确认外部用户或公开下游接入。

## 最小设计与选择

需要的能力：调用方已有 `right: Value` 且不再使用时，把新增/替换子树直接交给 RFC 操作，
无需复制这些子树。技术由现有 Palim/serde_json/json-patch 固定。
json-patch 4.2.0 的 [diff 源码](https://github.com/idubrov/json-patch/blob/v4.2.0/src/diff.rs)
确认其接口接收两个借用，新增和替换复制 payload。

选择 **adapt**：在临时库副本中复用现有深度检查、叶 delta 深度限制、数字规范化和 typed pointer，
用拥有所有权的 Map 迭代代替目标借用迭代。新增容器和字符串直接移动，Add 仍规范化数字，
Replace 仍保留目标数字表示。验证完整输入后才移动成员；任何中途错误返回 Error，
不返回部分补丁，也不会把部分移动的目标暴露给调用方。目标所有权在成功和失败时都会消耗。
输入深度错误发生时迭代销毁目标，避免极深 Value 的递归析构使错误变成栈溢出。

这个最小原型只支持新增和替换；删除、变化的 array/array 对、非排序 Map 返回明确的原型错误。
新增数组、整个容器替换及未变化数组均在支持范围。没有 move/copy 优化、tests、过滤或自定义匹配。
借用旧接口不能满足转移 payload 的目标；正式全功能 API、流式输出和并行不是本轮实施范围。
验证计划是支持输入与现有 plain 输出逐字节一致、独立正逆回放、数值/深度/所有权测试，
以及同输入上的 Core、调用前 clone 与完整 pipeline 测量。

## 测量方法

- 冻结基线 `ca2c28ab78b28a4ec92c25e63ba499f78d6a3e7a`，Apple M2 Max / macOS。
  Rust 版本、源码/原型/脚本/fixture hash、全部批次和进程中位数保存在
  [measurements.json](measurements.json)。复用 addition-copy 报告的五个原始 fixture，没有缩小输入。
- 两个借用控制组和原型使用同一依赖锁文件与 serde_json 精度特性；Palim 全部 RFC flag 关闭。
- **生成**：函数返回前打点；不含返回后销毁 Patch。owned 函数内部销毁未使用目标的成本仍在这里。
- **Core**：已解析 Values → 生成 → 销毁 Patch。owned-ready 的目标准备在计时外；owned-clone
  在计时内 clone 目标。借用控制组保留目标，因此 Core 比较是不同所有权合同，不是透明替换倍率。
- **Pipeline**：相同原始 UTF-8 → 解析两份文档 → 生成 → 序列化 → 销毁输入、输出及 Patch。
  这里没有在计时外准备 owned 目标，三种引擎有相同计时边界。
- 3 次预热，至少 10 ms 校准且上限 2048 次，9 批，3 个串行独立进程，逐轮轮换引擎顺序。
  表格取三个进程批次中位数的中位数。编译、输出验证在计时外。
- 60 个最终计时进程都独立应用序列化后的 Patch，再生成 inverse 并独立还原源文档；
  五个 fixture 的四种引擎及全部进程均输出相同 wire hash。

## add-wide-100000 的成本分解

单位 ms；输出均为一条 Add、4,288,932 B。

| 引擎/所有权 | 生成 | Core（包含 Patch 销毁） | Pipeline |
|---|---:|---:|---:|
| Palim 借用 | 22.4527 | 31.4808 | 84.0907 |
| json-patch 4.2.0 借用 | 14.3111 | 23.6138 | 79.0951 |
| owned-ready，已有目标 | 10.1645 | 19.5578 | 64.7727 |
| owned-clone，先复制目标 | 22.6278 | 31.7848 | 不另测 |

只计生成，owned 比现有 Palim 快约 2.21 倍，比 json-patch 快约 1.41 倍；
包含销毁后分别约 1.61 倍、1.21 倍。owned 仍需深度检查和新增数字规范化遍历，
输出销毁也没有消失。本轮没有对这两种遍历各自精确归因，不能声称它们分别占多少时间。
不能把更小输入的倍率或计时外准备目标说成 100k 的端到端倍率。

## 全部 fixture 的 Core

单位 ms，均包含 Patch 销毁。

| 输入 | Palim 借用 | json-patch 借用 | owned-ready | owned-clone |
|---|---:|---:|---:|---:|
| add-wide-10000 | 2.5630 | 2.0208 | 1.2216 | 2.3106 |
| add-wide-100000 | 31.4808 | 23.6138 | 19.5578 | 31.7848 |
| scalar-guards-100000（plain，无 tests） | 14.7323 | 30.8844 | 14.1408 | 17.5780 |
| unchanged-100000 | 0.6056 | 5.8339 | 1.6231 | 2.9128 |
| small-config-edit | 0.002524 | 0.002950 | 0.003064 | 0.003861 |

unchanged 的 consuming Core 明显更慢，因为它在返回前销毁拥有的目标；借用调用保留目标。
标量输入没有大型子树复制可以省掉，小配置也没有明显收益。不能为所有负载推荐 owned。

## 全部 fixture 的 Pipeline

单位 ms，计时内解析并销毁目标。各行 patch 字节一致。

| 输入 | Palim 借用 | json-patch 借用 | owned | Patch B |
|---|---:|---:|---:|---:|
| add-wide-10000 | 7.6069 | 7.1462 | 4.8313 | 418,932 |
| add-wide-100000 | 84.0907 | 79.0951 | 64.7727 | 4,288,932 |
| scalar-guards-100000（plain） | 75.3784 | 88.6258 | 72.8923 | 5,978,896 |
| unchanged-100000 | 11.1535 | 16.2998 | 11.1873 | 2 |
| small-config-edit | 0.009929 | 0.010270 | 0.009791 | 53 |

第一次未分解生成/销毁的 60 进程试测也完整保留在 measurements.json 的 `pilot` 中，
含当时原型和 runner 源码。那次 add-wide-100000 pipeline 为 86.93 / 75.98 / 58.00 ms，
owned Core 为 19.12 ms。最终批次没有挑选更快的试测数字。
这是共享机器上的 warm 单机测量；批次差异不能忽略，没有冷启动、并发、尾延迟或 RSS 结论。

## 已完成检查与决策

- 工作树：debug/release 各 201 个普通测试及 1 个 doctest 全部通过；fmt、Clippy 全目标
  warnings denied、Rust 1.85 library check 通过；ID 示例运行通过，每一步原生/RFC 回放和撤销/重做符合目标。
- 临时原型：4 个专项测试在 debug/release 下均通过；Clippy 全目标、Rust 1.85 library check 通过。
  测试包含 array backing storage 地址保持不变、转义键和容器替换、raw/unchecked 数字合同、
  124–129 层与原 plain 错误一致、100,000 层目标安全拒绝，以及不支持输入返回 Error。
- `cargo package --list --locked --allow-dirty`：列表示例被包含，scratchpad 和 results 被排除。
  这是包清单检查，不是 package 编译验证或发布。
- 未修改生产 src/Cargo.lock；未重新跑 JS、fuzz 或远端 CI。这些历史检查不冒充本轮结果。

**决策：保留原型和证据，暂不加入公开 API。** 已验证的收益是“已有目标、宽子树新增”的
较低 Core/pipeline 成本，不是最大众 RFC 输入全面快 4–7 倍。正式实现需要先确定具体 consuming
调用方与数组/优化支持范围；现有接口的性能基线本轮未改动。

## 复现

脚本只在临时目录注入模块并开放两项现有私有 helper 的 crate 内可见性，结束后删除临时目录。
仓库公开库接口保持不变。需要 Python 3.12+、已缓存 Cargo 依赖和 Rust 1.85.0/stable toolchain。

```sh
python3 scratchpad/run-owned-patch.py \
  --source-ref ca2c28ab78b28a4ec92c25e63ba499f78d6a3e7a \
  --output /tmp/palim-owned-replay.json
```

只做专项检查可加 `--verify-only`。复现会生成当前原型的测量，不自动重跑 `pilot`，也不发布或推送。
