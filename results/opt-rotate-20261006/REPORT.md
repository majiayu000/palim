# Pointer 与数组生成优化 — 2026-10-06

基线是已推到 origin/main 的 `6a8f4a0`，最终源码冻结在 measurements.json。保留深度、数字规范化与错误约定；没有新增 API、配置、依赖或 crate 版本。按 Pointer → 字节成本缓存 → 哈希器检查的顺序探索，每一步独立计时。

## 采用与撤掉的方案

- **采用 Pointer 读取改造。** factorize/rationalize、guard 失效判断和标准应用的内部读路径使用已有 jsonptr。未转义 token 可借用；真实错误仍映射到原错误。逆补丁的动态字符串路径不在这一改动范围内。
- **撤掉操作字节成本缓存原型。** 为 rationalize 建立每槽一次计数、候选复用并随接受操作压缩。10 万标量 opt 的相邻轮次仅改善约 1.5%，分配反增一次，迁移反增 12 次，未证明足够收益；完整原型和原始样本仍保留。最终源码不含它。
- **未替换哈希器。** 本机 gix-imara-diff 0.2.5 的 Interner 使用 hashbrown 0.15.5 DefaultHashBuilder，而它已是 foldhash::fast::RandomState。rotate 的 token 哈希并不是待替换的 SipHash。其他 std HashMap/HashSet 和 compare 的 DefaultHasher 不据此宣称已优化。
- **采用未变化匹配项的路径省略。** 原实现给每个匹配项创建路径，然后 node_unfiltered 发现值相同立即返回。提前检查相等，匹配/投影回调顺序保留，只有变化的项才构造路径；已有比较后分派复用，变化的项不重复深比较。
- **采用 LIS 递增快路。** 当 source index 大于最后一条 tail 时直接追加；其余保留原二分搜索、等值 tie 和重建选择。穷举 21,845 条短序列与原算法的完整 pair 输出完全相同。

## 分阶段测量

各阶段使用相同工具：3 个独立进程、每进程 3 次热身、10 ms 校准、9 批测量，串行且交替版本顺序。Core 是预解析 Values 的生成及输出销毁；Pipeline 包括同一紧凑输入字节的解析、生成、紧凑 UTF-8 序列化与销毁。生成结果在计时外由独立 json-patch 正向应用，并生成逆补丁恢复。自己的编译、测试和 fuzz 不与计时重叠；系统其他负载未隔离。

| 独立步骤 | 场景 / 模式 | 前 ms | 后 ms | 结论 |
|---|---|---:|---:|---|
| baseline-pointer | file_migrations_move_copy_5000 / optimized | 34.317 | 32.710 | 采用；少 40,004 次分配 |
| pointer-costs | scalar-guards-100000 / optimized | 162.177 | 159.762 | 收益小，撤掉 |
| pointer-paths | rotate-1000000 / plain | 162.989 | 127.907 | 采用；省 1,000,000 次路径分配 |
| paths-final | rotate-1000000 / plain | 128.719 | 89.842 | 采用；递增序列省二分搜索 |

每步骤另有 7 组控制对比，全部前后补丁 hash 一致。步骤在不同轮次执行，不能直接把这些绝对耗时相减。归档共 **282 个计时进程、40 份独立分配统计**；全部最终输出销毁后的存活字节增量为 0。

## 最终同轮对比

只有 plain 与 Rust json-patch 4.2.0 对齐生成功能边界，统一 serde_json 1.0.151 的 float_roundtrip/arbitrary_precision；工具为两版本配置相同的 object_hash 回调（读取 id）。opt 开启 factorize/rationalize，guard 另开 tests。三种补丁输出都独立验证，不能用 plain 竞品作 opt/guard 的同功能速度排名。

| 场景 / 模式 | 6a8f4a0 Core ms | 最终 Core ms | json-patch Core ms | 最终 Pipeline ms | json-patch Pipeline ms |
|---|---:|---:|---:|---:|---:|
| small-config-edit / plain | 0.0025 | 0.0025 | 0.0029 | 0.0098 | 0.0103 |
| file_migrations_move_copy_5000 / plain | 2.4283 | 2.4407 | 2.4901 | 12.9260 | 12.9258 |
| file_migrations_move_copy_5000 / optimized | 33.0493 | 31.8924 | — | 40.1300 | — |
| file_migrations_move_copy_5000 / guarded | 55.0296 | 53.4408 | — | 64.6015 | — |
| scalar-guards-100000 / plain | 13.1917 | 13.5090 | 29.6581 | 73.2409 | 86.8806 |
| scalar-guards-100000 / optimized | 158.2618 | 160.0111 | — | 215.2829 | — |
| scalar-guards-100000 / guarded | 202.0595 | 203.8576 | — | 263.3617 | — |
| rotate-1000000 / plain | 165.6533 | 96.9522 | 114.7392 | 174.2292 | 263.5595 |
| rotate-1000000 / optimized | 173.7188 | 92.9963 | — | 171.6215 | — |
| rotate-1000000 / guarded | 272.3988 | 193.1152 | — | 292.7982 | — |
| mixed-array-last-100000 / plain | 13.7067 | 13.5943 | 30.1618 | 73.3303 | 87.1586 |
| shuffle-2000 / plain | 1.5375 | 1.4369 | 0.2046 | 1.7566 | 0.5211 |

- 百万 rotate plain 的 Core 从 165.653 → 96.952 ms，约缩短 41.5%；同轮 Rust json-patch 为 114.739 ms。Pipeline 为 174.229 对 263.559 ms。Palim 补丁仍 44 B，json-patch 为 58,888,891 B。仅限这一输入与机器。
- 百万 rotate 的生成分配从 1,000,096 → 96 次；迁移 opt 从 473,666 → 433,662 次。Peak 是请求字节的额外存活量，不是 RSS。
- 10 万标量 opt 仍约 160.01 ms、2,131,738 次分配，尚未解决。guard 同轮从 202.06 → 203.86 ms，不能宣称这组改善。
- 随机 2,000 项重排虽从 1.538 → 1.437 ms，仍远慢于 json-patch 的 0.205 ms；Palim 的输出 81,181 B 对 87,736 B。这是剩余明确反例，不宣称全面领先。
- add-wide 未继续优化或重新计时。上一轮全 fixture 报告仍显示校验路径的剩余成本；本轮不采纳外部报告对该成本百分比的分解为自己的实测。

## 深层输入对照

独立子进程用循环直接构造两份 100,000 层嵌套数组，避免 json! 递归构造。相同 8 MiB 线程栈、release 和 serde 特性：Palim 默认深度校验返回 `<root>: JSON nesting exceeds max_depth`，退出码 0；json-patch diff 栈溢出，进程退出 -6（SIGABRT）。输入析构也递归，探针刻意 forget 两份输入，避免把构造/析构崩溃归因于生成器；这是生成入口的受控对照，不是任意操作或整个生命周期的安全保证。源码、完整 stderr 与命令条件在 measurements.json 的 depth 中。

## 实际完成的检查

- cargo fmt --check。
- cargo test --locked / cargo test --release --locked，各 **197** 项通过。
- cargo clippy --all-targets --locked -- -D warnings。
- cargo +1.85.0 check --lib --locked。
- release fixture_runner 与 JS 互通 **1,073** 案例，failures=[]。
- cargo +nightly fuzz run core，固定 seed 20261001、max_len=2048，**230,265** 次 / 61 秒，无失败。
- 冻结 oracle **3,305** 条，补丁、逆补丁、成本前缀、错误、正向/逆向结果全部与 6a8f4a0 字节一致。
- Pointer 回归覆盖空 key、斜线/波浪号、Unicode、缺失、非规范数组索引；原有匹配/过滤回调用例和数组移位 guard 回归通过。

本轮 CI 在本机完成，不据此宣称 GitHub 三平台 workflow 已完成。本轮未发布新 crate 版本。

## 重放

```sh
python3 results/opt-rotate-20261006/run.py --quick
python3 results/opt-rotate-20261006/run.py
```

脚本从前一份固定 evidence ZIP 读取已核对的 fixture、从 measurements.json 恢复每阶段冻结源码，并在新的临时目录编译。quick 只重放两组小型步骤和最终对比；完整模式重放全部 282 个计时进程。原始样本、哈希、源码、原型、锁文件、分配计数和验证日志见 [measurements.json](measurements.json)。

独立 quick 重放已完成：22 个计时进程、10 份分配结果；冻结源码/fixture 哈希、正向与逆向检查均通过，receipt 已归档。
