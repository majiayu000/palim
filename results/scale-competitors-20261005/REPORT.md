# Palim 大规模竞品对照（2026-10-05）

## 结论

本轮在同一台机器上完成 55 个输入/引擎组合，每组 3 个独立进程，共 165 条计时记录。四个竞品的版本已按查询当日官方注册表核对：Rust json-patch 4.2.0、Go jsondiff v0.7.1、JS fast-json-patch 3.1.1、JS jsondiffpatch 0.7.6。Palim 是上轮大规模测量使用的冻结未提交 guard 候选，基于已发布 0.1.4。没有修改库实现。

结果显示：Palim 的原生可逆数组 diff、移动识别和部分压缩路径有优势；大量字段普通 RFC 生成、逐字段保护和部分数组保护仍有落后。没有按不同协议、不同保护范围或跳过结果拼接一个总冠军。

## 关键对照：完整流程

以下均为相同输入字节的 **解析左右输入 → 生成 → 序列化输出**，单位 ms。原生格式与 RFC 6902 在各自对应行内比较。时间包含语言运行时的正常分配/回收成本，不是线上 P99。

| 场景 | Palim ms | 对照实现 ms | 输出和功能边界 |
|---|---:|---:|---|
| 10 万字段：普通 RFC | 142.835667 | rust-json-patch: 88.227583 | 同样 100000 条 Replace，约 5.979 MB |
| 10 万字段：逐字段保护 | 228.667125 | fast-json-patch-invertible: 110.159084 | 均 100000 Test + 100000 Replace，11.658 MB；路径之间独立，顺序不同 |
| 5000 对象迁移：压缩 RFC | 55.520083 | go-optimized: 113.955375 | Palim 0.505 MB / 5000 Move + 1 Copy；Go 4.699 MB / 2 条，输出不同 |
| 2 万项完全替换：压缩 RFC | 27.453167 | go-optimized: 1369.319708 | 规范化后的操作完全相同：1 条 Replace，0.340 MB |
| 2 万项完全替换：保护 RFC | 1654.388167 | go-optimized-invertible: 1383.156208 | 规范化后的操作完全相同：整数组 Test + Replace，0.680 MB |
| 百万项旋转：普通 RFC | 298.638125 | rust-json-patch: 267.176375 | Palim 1 条 Move / 44 B；对照 100 万条 Replace / 58.889 MB |
| 2000 项旋转：原生可逆 | 0.364837 | jsondiffpatch-native: 39.868000 | 规范化后的可逆 delta 相同，27 B |
| 2000 项完全替换：原生可逆 | 1.428818 | jsondiffpatch-native: 288.172625 | 规范化后的可逆 delta 相同，0.112 MB |
| 小配置：原生可逆 | 0.009194 | jsondiffpatch-native: 0.005948 | 规范化后的可逆 delta 相同，28 B |

## 已解析输入：关键对照

直接对应上一报告的生成阶段，不含解析和最终序列化。Rust 两方都返回拥有值的 typed Patch；跨语言比较仍受 GC 和结果所有权差异影响，不能直接解释为纯算法倍率。

| 场景 | Palim core ms | 对照 core ms | 结果边界 |
|---|---:|---:|---|
| 10 万字段普通 RFC | 82.598166 | rust-json-patch: 30.594250 | 同样 100000 Replace |
| 5000 对象：不压缩 RFC | 11.951750 | rust-json-patch: 2.649084 | 同样 5001 Add + 5000 Remove |
| 2 万项完全替换：不压缩 RFC | 2.367583 | rust-json-patch: 2.121505 | 同样 20000 Replace |
| 百万项旋转 | 221.850708 | rust-json-patch: 117.566459 | 1 Move 与 100 万 Replace，结果形状不同 |
| 10 万字段逐字段保护 | 164.638250 | fast-json-patch-invertible: 42.589875 | 相同 Test/Replace 数量，运行时和路径顺序不同 |
| 2000 项旋转：可逆 delta | 0.205461 | jsondiffpatch-native: 43.425792 | 规范化的 delta 相同，所有权与 GC 不同 |

## RFC 全矩阵

每个单元格为 ms / 序列化字节；plain 关闭 factorize/rationalize/tests；optimized 开启 factorize/rationalize。Go optimized 使用 CompareJSON + Factorize + Rationalize + SkipCompact（输入已是紧凑 JSON），默认未开启 LCS。Rust json-patch 和 fast-json-patch 默认按位置生成数组差异。补丁序列不同但都通过独立目标验收，不能把移动识别、补丁压缩和最少生成成本当成同一目标。

| 输入 | Palim plain | Palim optimized | Rust json-patch | Go default | Go optimized | JS fast-json-patch |
|---|---:|---:|---:|---:|---:|---:|
| small-config-edit | 0.009620 / 53 | 0.010861 / 53 | 0.010401 / 53 | 0.016472 / 53 | 0.025523 / 53 | 0.003348 / 53 |
| scalar-guards-100000 | 142.835667 / 5,978,896 | 291.543167 / 1,978,940 | 88.227583 / 5,978,896 | 151.399417 / 5,978,896 | 预算跳过，无完成值 | 89.133125 / 5,978,896 |
| file_migrations_move_copy_5000 | 23.174458 / 5,169,266 | 55.520083 / 505,072 | 13.838250 / 5,169,266 | 75.621333 / 5,169,266 | 113.955375 / 4,699,005 | 23.470292 / 5,169,266 |
| rotate-2000 | 0.385887 / 42 | 0.383751 / 42 | 0.489368 / 112,891 | 1.805000 / 112,891 | 14.698708 / 34,038 | 0.364210 / 112,891 |
| rotate-1000000 | 298.638125 / 44 | 297.885292 / 44 | 267.176375 / 58,888,891 | 818.205916 / 58,888,891 | 预算跳过，无完成值 | 318.792125 / 58,888,891 |
| disjoint-20000 | 5.163959 / 1,148,891 | 27.453167 / 340,038 | 4.917667 / 1,148,891 | 16.453958 / 1,148,891 | 1369.319708 / 340,038 | 4.275500 / 1,148,891 |

## 保护范围核对

为了避免把生成少量弱 Test 当成更强保护的性能提升，本轮独立验证器额外做了一个按输入形状选择的漂移实验：标量对象额外加未修改字段；数组在尾部加一个元素；迁移文档改 unchanged_notes。该有限实验只说明下表具体输入和具体漂移，不证明完整安全范围。

| 输入 | 模式 | pipeline ms | 输出 B / 操作数 | 该漂移是否被拒绝 |
|---|---|---:|---:|---|
| scalar-guards-100000 | palim-plain-guarded | 228.667125 | 11,657,786 / 200,000 | 否 |
| scalar-guards-100000 | go-invertible | 210.965875 | 11,657,786 / 200,000 | 否 |
| scalar-guards-100000 | fast-json-patch-invertible | 110.159084 | 11,657,786 / 200,000 | 否 |
| file_migrations_move_copy_5000 | palim-guarded | 94.745750 | 9,398,362 / 2 | 是 |
| file_migrations_move_copy_5000 | go-optimized-invertible | 139.486958 | 9,398,362 / 2 | 是 |
| file_migrations_move_copy_5000 | fast-json-patch-invertible | 36.107875 | 10,043,156 / 15,001 | 否 |
| disjoint-20000 | palim-guarded | 1654.388167 | 680,072 / 2 | 是 |
| disjoint-20000 | go-optimized-invertible | 1383.156208 | 680,072 / 2 | 是 |
| disjoint-20000 | fast-json-patch-invertible | 6.648875 | 2,237,781 / 40,000 | 否 |

10 万字段输入：Palim plain-guarded、Go Invertible 和 fast-json-patch invertible 都逐字段 Test/Replace，未额外认证未修改的新增字段。三个模式的输出大小和 Test/Replace 数量相同；fast-json-patch 的路径顺序不同。
2 万项数组输入：Palim guarded 和 Go optimized-invertible 的规范化操作序列完全一致，都认证整个原数组；fast-json-patch 只认证被替换的位置，不拒绝额外尾元素，尽管它也能恢复本轮无漂移的目标。不能用其较小耗时宣称等价整数组保护更快。
5000 迁移输入：Palim guarded 与 Go optimized-invertible 的规范化操作序列也完全一致，都是整文档 Test + Replace；fast-json-patch 的其他字段保护范围不同。
Go 的 Invertible 选项是给正向 Remove/Replace 加 Test，并不等于本库的受保护逆向 API。jsondiffpatch reverse 属于原生 delta。本轮没有给前一报告中的 invert_json_patch_guarded 107.7 ms 找一个不同功能的替代值排名。

## 原生可逆格式

两者都使用 object_hash/objectHash 取 id，字符串数组没有 id。每个进程都用自己的 patch 和 reverse 恢复输入。默认 JS delta 可能保留输入引用，Palim 返回拥有值的 Delta；core API 的结果所有权不同。完整流程把结果序列化后用于更接近的实际流程对照。

| 输入 | Palim pipeline ms / B | jsondiffpatch pipeline ms / B | 规范化 delta |
|---|---:|---:|---|
| small-config-edit | 0.009194 / 28 | 0.005948 / 28 | 相同 |
| scalar-guards-100000 | 105.556750 / 2,767,797 | 118.202292 / 2,767,797 | 相同 |
| file_migrations_move_copy_5000 | 21.387375 / 9,433,139 | 22.224417 / 9,433,139 | 相同 |
| rotate-2000 | 0.364837 / 27 | 39.868000 / 27 | 相同 |
| disjoint-2000 | 1.428818 / 111,790 | 288.172625 / 111,790 | 相同 |
| rotate-1000000 | 291.545666 / 29 | 预算预检跳过 | 无对照结论 |
| disjoint-20000 | 15.580459 / 1,157,790 | 预算预检跳过 | 无对照结论 |

## 测量方法和预算

- Apple M2 Max / macOS arm64，Rust 1.97.0、Node 24.14.0；精确 Go 版本等信息见 environment。各进程串行执行，重复轮次轮换引擎顺序；没有并行跑多个 benchmark。系统其他活动未隔离。
- 每个已完成组合 3 个独立进程；每个指标 3 次预热，至少 10 ms 自适应校准（最多 2048 次），9 个批次。主表为三个进程中位数的中位数，不是所有请求的 P99。原始样本、每进程中位数均保留。
- 两个指标：core 已解析输入 → 公共生成 API；pipeline 解析同样 left_raw/right_raw 字节 → 生成 → 序列化。Rust core 包括输出销毁；JS/Go 通过正常 GC 回收输出，不强制在计时内 GC。Go 在 core 与 pipeline 之间调用一次 GC，计时外。Node pipeline 输出字符串，Rust/Go 输出 UTF-8 Vec/byte slice；本轮输入和输出全为 ASCII，字节/字符长度相等，但运行时表示仍有差别。
- Go optimized 的 core 未计时：CompareWithoutMarshal 缺少 Rationalize 使用的 targetBytes/valueLen，会与 CompareJSON 的优化选择不同；给它使用一个更廉价但不同的调用路径会误导。Go default/Invertible 的 core 已计时，具体记录单列。
- 所有输入均为合成负载，数字处于 JS/Go 能精确表示的整数范围；没有测大整数精度、Unicode、过滤规则、错误恢复、深度限制、并发或网络传输。不能由速度表推导完整功能排名。
- RFC 每个不同输出由 Rust json-patch 应用到同一 baseline 校验目标，再由 Palim 生成 verification inverse 并恢复原值；这只验收生成结果，没有假称各竞品都提供逆向生成 API。所有 165 条记录的输出均完成对应正向/逆向核验；各组合三轮输出 SHA 相同。
- jsondiffpatch 的 root 数组超过 2000 项在执行前按事先 LCS 矩阵预算跳过；源码初始化完整 (n+1)×(m+1) 矩阵。百万项旋转和 2 万项全异不能填入一个虚构耗时，也没有实测 OOM 结论。
- Go optimized 的 10 万字段和百万项旋转第一次完整探针达到 60 秒进程预算，后两轮不再重复。预算覆盖初始生成、预热、采样等完整探针，不等于单次生成已经耗时超过 60 秒。此两项没有完成值，不把 timeout 当正确性失败或慢多少倍。
- 实际 55 个完成组合 × 3 进程 = 165 条记录；12 条跳过记录（含两次 timeout 与后续取消重复、JS 预检跳过）；没有非预算执行失败。

## 对性能差距的解释

**已核对源码：** Palim 普通对象 RFC 路径会先生成可逆 native delta 再导出标准 patch；json-patch 直接递归生成标准操作。大量简单字段时 Palim 有额外中间值和遍历。完全不相交的原始标量数组在无 tests 时已有直出快路径，不能把所有输入都说成必经完整 native 中间层。

**已核对源码：** Palim guarded 数组成本估算反复序列化父数组，上一报告已显示二次增长。本轮同样 Test+Replace 的结果比 Go 慢，说明这仍是可测的缺口。

**推断：** 普通对象直出 RFC 的限定快路径，以及数组父快照成本复用，是接下来最值得验证的两个方向。本轮没有 CPU profile，也没有把这些解释写成已测出的精确耗时占比或已实现的修复。

**协议权衡：** 百万项旋转的 Palim 生成只输出 44 B，位置式实现输出 58.9 MB；生成阶段几毫秒差异不能替代传输/存储成本的单独测量。本轮没有计算网络带宽或实际应用吞吐量。

## 原始证据与复现

`measurements.json` 包含原始环境、165 条记录、55 个汇总、预算记录与官方版本查询。`evidence.zip` 包含冻结源码、Rust/Go/JS 探针、依赖锁文件、输入和每个不同 RFC 补丁的原始 wire、SHA 清单。Rust/Go 二进制哈希和 Node 运行依赖文件哈希保留在环境中，包内不包含编译产物或 node_modules。

解压后 `python3 rebuild.py . /tmp/palim-competitors-rerun`，然后进入新目录执行 `python3 run.py`。需要 macOS sysctl（环境读取）、Rust/Cargo、Go、Node/npm 和锁文件依赖。所有复测进程仍串行。原报告的 measurements 目录不会覆盖。

源码依据：[Palim RFC 入口](../../src/lib.rs)、[成本估算](../../src/rfc.rs)、[json-patch diff](https://docs.rs/json-patch/4.2.0/src/json_patch/diff.rs.html)、[Go v0.7.1 compare](https://github.com/wI2L/jsondiff/blob/v0.7.1/compare.go)、[Go Rationalize](https://github.com/wI2L/jsondiff/blob/v0.7.1/differ.go)、[Go Invertible 文档](https://github.com/wI2L/jsondiff#invertible-patch)、[JS invertible 文档](https://github.com/Starcounter-Jack/JSON-Patch#jsonpatchcomparedocument1-document2-invertible)、[JS LCS 源码](https://unpkg.com/jsondiffpatch@0.7.6/lib/filters/lcs.js)。

本轮没有修改库实现，没有重新运行上一轮完整测试/Clippy/fuzz，没有提交或发布；完成的是有独立验收的大规模竞品测量。

便携复现已实际验证：从证据包重新构建 Rust/Go 探针，并 npm ci 恢复 JS 依赖；运行依赖文件哈希全部与原测量一致。重放小配置的 8 个引擎，规范化输出与原始测量完全相同，正向/逆向验收通过。未重复完整竞品计时矩阵。
