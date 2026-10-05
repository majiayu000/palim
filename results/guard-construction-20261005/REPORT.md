# Palim：直接构造 guard 操作（2026-10-05）

## 改动范围

基线是已发布 0.1.4（`50c72d59fd02d8a0e642100ad30be0771fb4eab0`），全部 10 个 Rust 模块逐字节核对。候选以冻结源码哈希标识；本轮改动尚未提交或发布。

生产改动仅位于 `guard`：直接创建 `PatchOperation::Test(TestOperation { path, value })`，省去含 op/path/value 的外层 JSON Map、字符串键和拆解。继续使用同一 PointerBuf parser、同一错误映射、原 guard_values 查询、原操作顺序和实际 apply。没有新增 API、依赖、unsafe、配置或缓存。

payload 保留原 `json!(value)` 序列化，未改成 `value.clone()`。使用 serde_json 的 unchecked Number 构造器提供合法但未规范化的表示时，原行为包括 `-0 → 0`、`1E+0003 → 1e+0003`、`1.00e00 → 1.00e+00`。直接 clone 会改变 guard 字节；基线探针及新增永久回归记录了该边界。此轮不改变这些语义。

## 测量边界

39 个输入/模式，分别冻结源码与锁文件。每版 3 个独立进程、交替版本顺序；耗时使用默认分配器二进制，分配量使用另一计数二进制，共 468 条记录。每进程 3 次热身、至少 10 ms 校准（上限 2048 次）、9 个批次；表格为进程中位数的中位数。

计时包括已解析 Value / typed Patch → 公共 diff 或 inverse API → 输出销毁；不含解析、最终序列化和验证。plain 关闭 factorize/rationalize；optimized 开启两者；guarded 额外开启 tests。inverse 调用 invert_json_patch，inverse-guarded 调用 invert_json_patch_guarded，并在计时前准备原始 typed Patch。

原 27 个对照组合之外增加 100/2000 项 scalar Replace 和 128 次数组 Remove。数组 Remove 的 inverse 会在插入前认证父数组，包含较大的父快照；输入命名不是所有模式都包含父快照。全部输出哈希与基线一致，独立 json-patch 正向与逆向核验在计时外完成。输入为合成负载，没有新增竞品测量。

## 完整性能矩阵

| 输入 | 模式 | 0.1.4 ms | 候选 ms | 比值 |
|---|---|---:|---:|---:|
| small-config-edit | optimized | 0.003151 | 0.003122 | 1.009× |
| small-config-edit | guarded | 0.007290 | 0.007096 | 1.027× |
| rotate-2000 | optimized | 0.225471 | 0.225490 | 1.000× |
| rotate-2000 | guarded | 0.407021 | 0.406151 | 1.002× |
| disjoint-2000 | optimized | 2.340375 | 2.351646 | 0.995× |
| disjoint-2000 | guarded | 20.415500 | 20.492458 | 0.996× |
| ambiguous-ids-2000 | optimized | 6.489375 | 6.465604 | 1.004× |
| ambiguous-ids-2000 | guarded | 8.403917 | 8.384250 | 1.002× |
| file_migrations_move_copy_100 | factorize | 0.664234 | 0.658969 | 1.008× |
| file_migrations_move_copy_100 | optimized | 0.720036 | 0.719138 | 1.001× |
| file_migrations_move_copy_100 | guarded | 1.299010 | 1.285208 | 1.011× |
| file_migrations_move_copy_400 | factorize | 2.742719 | 2.720927 | 1.008× |
| file_migrations_move_copy_400 | optimized | 2.951667 | 2.956198 | 0.998× |
| file_migrations_move_copy_400 | guarded | 5.320688 | 5.224375 | 1.018× |
| file_migrations_move_copy_800 | factorize | 5.579979 | 5.515000 | 1.012× |
| file_migrations_move_copy_800 | optimized | 5.987396 | 5.955208 | 1.005× |
| file_migrations_move_copy_800 | guarded | 10.809875 | 10.679000 | 1.012× |
| duplicate-object-migrations-100 | factorize | 0.511828 | 0.510508 | 1.003× |
| duplicate-object-migrations-100 | optimized | 0.561167 | 0.561801 | 0.999× |
| duplicate-object-migrations-100 | guarded | 0.979479 | 0.974440 | 1.005× |
| duplicate-object-migrations-800 | factorize | 4.321177 | 4.280583 | 1.009× |
| duplicate-object-migrations-800 | optimized | 4.634989 | 4.670271 | 0.992× |
| duplicate-object-migrations-800 | guarded | 8.157833 | 8.218666 | 0.993× |
| independent_add_remove_replace_800 | optimized | 10.423958 | 10.414583 | 1.001× |
| independent_add_remove_replace_800 | guarded | 13.210125 | 13.337625 | 0.990× |
| root_add_guard_overlap_800 | optimized | 10.354500 | 10.407541 | 0.995× |
| root_add_guard_overlap_800 | guarded | 14.423375 | 14.358042 | 1.005× |
| scalar-guards-100 | plain | 0.049679 | 0.049816 | 0.997× |
| scalar-guards-100 | plain-guarded | 0.132226 | 0.112934 | 1.171× |
| scalar-guards-100 | inverse | 0.079391 | 0.079070 | 1.004× |
| scalar-guards-100 | inverse-guarded | 0.155027 | 0.137788 | 1.125× |
| scalar-guards-2000 | plain | 1.226646 | 1.222344 | 1.004× |
| scalar-guards-2000 | plain-guarded | 2.958125 | 2.583781 | 1.145× |
| scalar-guards-2000 | inverse | 1.667510 | 1.678354 | 0.994× |
| scalar-guards-2000 | inverse-guarded | 3.287500 | 2.919979 | 1.126× |
| array-parent-guards-128 | plain | 0.091615 | 0.092405 | 0.991× |
| array-parent-guards-128 | plain-guarded | 0.178597 | 0.151699 | 1.177× |
| array-parent-guards-128 | inverse | 0.085803 | 0.085175 | 1.007× |
| array-parent-guards-128 | inverse-guarded | 1.703146 | 1.688703 | 1.009× |

本组最低比值 0.990×，未观察到超过 5% 的回退。固定矩阵与短进程计时不能证明任意输入无回退或服务 P99 改善。

2000 项 scalar guarded diff 耗时减少约 12.65%，guarded inverse 减少约 11.18%；100 项对应约 14.59% / 11.12%。800 项大 payload 迁移 guarded 基本持平，不能把 scalar 收益推广到大 payload。

## 分配请求与峰值

| 输入/模式 | 分配/重分配次数：基线 → 候选 | 累计请求 B：基线 → 候选 | 峰值额外 live B：基线 → 候选 |
|---|---:|---:|---:|
| file_migrations_move_copy_800 / guarded | 181,545 → 181,540 | 23,164,628 → 23,163,981 | 9,515,236 → 9,515,236 |
| scalar-guards-100 / plain-guarded | 3,364 → 2,864 | 172,406 → 107,706 | 63,505 → 62,886 |
| scalar-guards-100 / inverse-guarded | 4,139 → 3,639 | 228,000 → 163,300 | 44,052 → 43,429 |
| scalar-guards-2000 / plain-guarded | 66,710 → 56,710 | 3,235,258 → 1,941,258 | 1,143,877 → 1,143,260 |
| scalar-guards-2000 / inverse-guarded | 82,368 → 72,368 | 4,357,607 → 3,063,607 | 765,757 → 765,134 |
| array-parent-guards-128 / plain-guarded | 4,829 → 4,189 | 263,653 → 180,837 | 66,577 → 65,938 |
| array-parent-guards-128 / inverse-guarded | 77,655 → 77,015 | 1,567,745 → 1,484,929 | 891,005 → 890,368 |

2000 项 scalar guarded diff 累计请求减少约 40.0%，guarded inverse 减少约 29.7%；每项 guard 少 5 次分配/重分配请求。峰值仅减少约 0.6 KB。累计请求、同时存活的请求字节与 RSS 是不同指标；本轮不声称进程峰值内存减少 40%。全部 final_live_delta 为 0。

## 实际完成的验证

- Debug/release 各 179 项测试；fmt、diff check、Clippy all-targets -D warnings、Rust 1.85 lib 与 release examples 通过。
- JS 必需互通 1,073 例通过；上游自逆向失败和畸形文本头保持单列。
- 两版独立重新编译：公共 3,040 条、私有 34 条、原专项 152 条、快照专项 43 条、构造专项 36 条，共 3,305 条完整记录逐字节一致。
- 构造专项覆盖 6 种合法但未规范化/大整数/大指数的 Number 表示 × 6 种操作；比较 guard 与 guarded inverse 的字节，并用本库数学数值 test 校验 guard 应用。该专项不把 json-patch 的表示相等当成数学数值相等，也不要求 unchecked 原始表示被 inverse 原样恢复。
- 两个新增永久回归在基线与候选均通过：数值规范化和转义路径、缺失来源/父节点/原始应用错误次序。
- 最终 nightly fuzz：Done 232410 runs in 61 second(s)；未崩溃，有限运行不代表完整覆盖。
- 新增 Criterion guard-construction group 实际运行通过；正向/逆向核验在计时外。
- 对照工具首次准备漏带了 manifest 声明的 benches，编译尚未开始即报错；补齐后重新编译并完成全部对照。失败的准备记录保留在证据包。

## 剩余成本与复测

大 payload 的递归序列化、最终 test 内容复制，以及依赖候选的完整文档重放仍然存在；本轮只删除操作外壳，未改变父快照策略。下一步可研究复杂 Move/数组的成本复用，先验证依赖边界和错误语义。

解压 evidence.zip，在 benchmark 目录执行 `python3 runner.py sources/baseline sources/candidate rerun fixtures`；在 oracle 目录执行 `python3 run.py` 可重新编译字节对照。需要锁文件对应的 Cargo 依赖；oracle hooks 和冻结输入均保留。
