# Palim：连续对象重命名的父快照成本（2026-10-04）

## 基线与范围

基线是上一轮 Move 批处理完成后的本地源码，逐文件核对与 [Move 优化报告](../move-costs-20261004/REPORT.md) 的候选冻结源码相同；测量时两轮改动均未提交，本报告不代表发布了新的 crate 版本。不是已发布 0.1.3 与本轮的直接性能对照。

生产改动仅位于 guarded_operation_bytes：保留一个父节点 test 操作的精确字节数。连续 Move 的来源与目标必须同属 Object 父节点，且目标成员不存在；资格由原 guard_values 查询结果判定，没有增加另一层文档探测。

合格 Move 不改变成员数、值、冒号或逗号数量。下一份父快照 test 的长度 = 当前精确长度 − 来源键 JSON 字符串长度 + 目标键 JSON 字符串长度。使用 Pointer 最后一个 token 的 decoded 字符串，再以同一 serde_json writer 计数；包含 UTF-8、引号、控制字符、空键和 ~01。

源 guard 和操作自身继续逐次序列化，原始实际 apply 顺序不变；apply 成功后才保存下一份缓存。任何其他操作清空缓存；不同父路径重新计价。前缀重放后从空缓存开始。根对象的成员重命名可使用缓存，整个 root 的移动不能。

没有新增公开 API、依赖、配置或 unsafe，没有更改最终生成的 test 内容。独立静态审阅未发现反例，这不是形式化证明。

## 测量方法

与上一轮同一组 27 个输入/模式，分别冻结两份源码与锁文件。每版 3 个独立进程、版本交替运行；耗时与分配量用不同二进制，共 324 条记录。每进程 3 次热身、至少 10 ms 校准（上限 2048 次）、9 个批次；表格取进程中位数的中位数。

计时边界：已解析两份 Value → 公共 RFC diff API → 输出销毁。不含解析、最终序列化及验证。factorize 关闭 rationalize；optimized 开启两者；guarded 再开启保护测试。

所有测量输出哈希相同，经独立 json-patch 正向应用和生成逆向补丁恢复。迁移输入是合成文件清单，不是生产事件。没有新增跨语言竞品排名。

## 完整性能矩阵

| 输入 | 模式 | 上轮 ms | 本轮 ms | 提速 |
|---|---|---:|---:|---:|
| small-config-edit | optimized | 0.003213 | 0.003211 | 1.000× |
| small-config-edit | guarded | 0.007407 | 0.007386 | 1.003× |
| rotate-2000 | optimized | 0.227193 | 0.228223 | 0.995× |
| rotate-2000 | guarded | 0.407921 | 0.408707 | 0.998× |
| disjoint-2000 | optimized | 2.389948 | 2.371422 | 1.008× |
| disjoint-2000 | guarded | 20.681833 | 21.128125 | 0.979× |
| ambiguous-ids-2000 | optimized | 6.487459 | 6.449521 | 1.006× |
| ambiguous-ids-2000 | guarded | 8.437167 | 8.482854 | 0.995× |
| file_migrations_move_copy_100 | factorize | 0.675615 | 0.677901 | 0.997× |
| file_migrations_move_copy_100 | optimized | 0.730805 | 0.734734 | 0.995× |
| file_migrations_move_copy_100 | guarded | 5.087688 | 1.322667 | 3.847× |
| file_migrations_move_copy_400 | factorize | 2.800740 | 2.806250 | 0.998× |
| file_migrations_move_copy_400 | optimized | 3.003042 | 3.003229 | 1.000× |
| file_migrations_move_copy_400 | guarded | 64.835167 | 5.427875 | 11.945× |
| file_migrations_move_copy_800 | factorize | 5.701666 | 5.638833 | 1.011× |
| file_migrations_move_copy_800 | optimized | 6.073875 | 6.153583 | 0.987× |
| file_migrations_move_copy_800 | guarded | 249.079084 | 11.046917 | 22.547× |
| duplicate-object-migrations-100 | factorize | 0.514561 | 0.520626 | 0.988× |
| duplicate-object-migrations-100 | optimized | 0.567471 | 0.565909 | 1.003× |
| duplicate-object-migrations-100 | guarded | 4.700844 | 0.990703 | 4.745× |
| duplicate-object-migrations-800 | factorize | 4.383302 | 4.391177 | 0.998× |
| duplicate-object-migrations-800 | optimized | 4.837938 | 4.846875 | 0.998× |
| duplicate-object-migrations-800 | guarded | 249.289625 | 8.446854 | 29.513× |
| independent_add_remove_replace_800 | optimized | 10.629917 | 10.680750 | 0.995× |
| independent_add_remove_replace_800 | guarded | 13.697916 | 13.510916 | 1.014× |
| root_add_guard_overlap_800 | optimized | 10.588792 | 10.587709 | 1.000× |
| root_add_guard_overlap_800 | guarded | 14.808250 | 14.775083 | 1.002× |

本组最低提速比为 0.979×；固定矩阵不能证明所有输入无回退。

## 分配请求

| guarded 输入 | 累计请求：上轮 → 本轮 B | 峰值额外 live：上轮 → 本轮 B |
|---|---:|---:|
| file_migrations_move_copy_800 | 23,164,628 → 23,164,628 | 9,515,236 → 9,515,236 |
| duplicate-object-migrations-800 | 22,935,857 → 22,935,857 | 9,369,374 → 9,369,374 |

这些是请求字节，不是 RSS。全部 final_live_delta 为 0；缓存保存路径引用与一个 usize，没有复制父文档。

## 验证实际完成

- Debug 177 项、release 177 项；fmt、diff check、Clippy all-targets -D warnings、Rust 1.85 lib 与 release examples 通过。
- 两版新编译的对照：公共 3,040 条、私有 34 条、原专项 152 条、新成本专项 43 条，共 3,269 条记录逐字节一致。
- 新成本专项 40 个有效序列及 3 个错误序列；每个有效序列的每个前缀都与真正生成并序列化的 guard 补丁核对，并从该前缀重放后重新计价；同时用独立 json-patch 正向应用和逆向恢复。
- 两个永久回归同时在上轮与本轮源码通过：精确前缀/前缀续跑、缓存失效/错误优先次序。覆盖根、稳定数组祖先、转义/Unicode/空键、大整数/1.0/1e9999、覆盖、Copy、数组 Move、其他写入及 Test。
- JS 必需互通 1,073 例通过；上游自逆向失败和畸形文本头单独记录。
- 最终 nightly fuzz：Done 181932 runs in 61 second(s)；未崩溃，有限运行不代表完整覆盖。
- 已有 Criterion 对象迁移 benchmark 实际重跑通过。

## 剩余成本

连续同父对象重命名的成本改善不涵盖数组位移、目标覆盖、跨父迁移和任意交错写入。保守清空一个缓存避免维护依赖图；不同父节点频繁交替仍需重新序列化。最终 guard 若确实包含完整父对象，输出复制和序列化成本仍然存在。

本轮专注父快照计价；补丁合并功能仍是独立后续任务。

复测：解压 evidence.zip，在 benchmark 目录执行 `python3 runner.py sources/baseline sources/candidate rerun fixtures`。需要锁文件对应的 Cargo 依赖。
