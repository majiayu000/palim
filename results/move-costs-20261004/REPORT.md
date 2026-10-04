# Palim：独立对象 Move 批处理（2026-10-04）

## 实现与证明边界

基线是已发布的 0.1.3（`7646a6f676d3c5c00dcc462433f8cfcded2e487c`）。候选源码及每个 Rust 文件的 SHA-256 保存在证据包；本报告不代表已经发布新版本。

保留原始完整 replay 和第一个候选的真实 replay；后者检查 source 默认深度、每一步操作合法性和数学目标相等。只有第一个候选成功后才检查批量独立性，避免无 Move 输入承担整批检查。

只接纳目的位置不重复、互不为祖先、原始父节点为 Object 的 Add/Remove/Replace。稳定的数组祖先允许存在；直接数组写入、重复位置、祖先写入、Move/Copy/Test 继续走已有通用流程。

来源值仍按 Value 表示相等匹配，Remove 队列和 Add 遍历保持原始顺序，相同的 UTF-8 字节收益门限保持不变。独立配对不改变其他操作读写，已验证候选的目标相等可以传递给后续配对。最终只压缩一次操作向量；VecDeque 避免重复 payload 的队首删除退化为移动整个来源列表。

本轮生产代码只改变 RFC factorize，不新增公开 API、依赖、unsafe 或配置。静态独立审阅未发现反例；这不是形式化证明。

## 测量方法

27 个输入/模式组合，每版 3 个独立进程；耗时与分配量使用不同二进制，版本交替运行，共 324 条记录。每个进程 3 次热身、至少 10 ms 校准（上限 2048 次）、9 个批次；表格取进程中位数的中位数。

耗时边界：已经解析的两份 Value → 公共 RFC diff API → 输出销毁。不含 JSON 解析、最终序列化和正确性验证。factorize 模式关闭 rationalize；optimized 开启两者；guarded 再开启保护测试。

迁移输入是合成文件清单，不是生产事件或真实用户数据。所有测量输出哈希与基线相同，均经独立 json-patch 正向应用及生成逆向补丁恢复；没有重新比较跨语言竞品。

## 完整性能矩阵

| 输入 | 模式 | 0.1.3 ms | 候选 ms | 提速 |
|---|---|---:|---:|---:|
| small-config-edit | optimized | 0.003164 | 0.003224 | 0.981× |
| small-config-edit | guarded | 0.007419 | 0.007421 | 1.000× |
| rotate-2000 | optimized | 0.224755 | 0.226907 | 0.991× |
| rotate-2000 | guarded | 0.408971 | 0.408474 | 1.001× |
| disjoint-2000 | optimized | 2.370802 | 2.372417 | 0.999× |
| disjoint-2000 | guarded | 20.664375 | 20.849541 | 0.991× |
| ambiguous-ids-2000 | optimized | 6.690958 | 6.520313 | 1.026× |
| ambiguous-ids-2000 | guarded | 8.482729 | 8.477521 | 1.001× |
| file_migrations_move_copy_100 | factorize | 25.733208 | 0.690547 | 37.265× |
| file_migrations_move_copy_100 | optimized | 25.734708 | 0.741620 | 34.701× |
| file_migrations_move_copy_100 | guarded | 29.676750 | 5.028396 | 5.902× |
| file_migrations_move_copy_400 | factorize | 413.703333 | 2.816344 | 146.894× |
| file_migrations_move_copy_400 | optimized | 404.318208 | 3.028469 | 133.506× |
| file_migrations_move_copy_400 | guarded | 483.144042 | 65.717084 | 7.352× |
| file_migrations_move_copy_800 | factorize | 1685.795959 | 5.718604 | 294.792× |
| file_migrations_move_copy_800 | optimized | 1657.142084 | 6.045208 | 274.125× |
| file_migrations_move_copy_800 | guarded | 1928.170542 | 247.938041 | 7.777× |
| duplicate-object-migrations-100 | factorize | 18.148500 | 0.517214 | 35.089× |
| duplicate-object-migrations-100 | optimized | 18.145250 | 0.563779 | 32.185× |
| duplicate-object-migrations-100 | guarded | 22.337708 | 4.716614 | 4.736× |
| duplicate-object-migrations-800 | factorize | 1198.919292 | 4.370188 | 274.340× |
| duplicate-object-migrations-800 | optimized | 1220.311083 | 4.713063 | 258.921× |
| duplicate-object-migrations-800 | guarded | 1420.330375 | 247.364750 | 5.742× |
| independent_add_remove_replace_800 | optimized | 10.584666 | 10.562208 | 1.002× |
| independent_add_remove_replace_800 | guarded | 13.627917 | 13.557208 | 1.005× |
| root_add_guard_overlap_800 | optimized | 10.820084 | 10.682458 | 1.013× |
| root_add_guard_overlap_800 | guarded | 14.523333 | 14.340416 | 1.013× |

这 27 个固定组合的最低提速比为 0.981×，没有超过 5% 的耗时回退；这不是所有输入无回退的证明。

## 分配请求（不是 RSS）

| 输入/模式 | 累计请求：基线 → 候选 B | 峰值额外 live：基线 → 候选 B |
|---|---:|---:|
| file_migrations_move_copy_800 / factorize | 3,886,465,841 → 12,250,841 | 9,507,044 → 9,515,236 |
| file_migrations_move_copy_800 / optimized | 3,886,769,369 → 12,554,369 | 9,507,044 → 9,515,236 |
| file_migrations_move_copy_800 / guarded | 3,897,379,628 → 23,164,628 | 9,507,044 → 9,515,236 |
| duplicate-object-migrations-800 / factorize | 3,813,627,031 → 12,049,568 | 8,178,570 → 8,178,602 |
| duplicate-object-migrations-800 / optimized | 3,813,914,477 → 12,337,014 | 8,178,570 → 8,178,602 |
| duplicate-object-migrations-800 / guarded | 3,824,513,320 → 22,935,857 | 9,369,374 → 9,369,374 |

所有分配测量的 final_live_delta 为 0。请求字节反映累计分配请求，不等于实际内存占用或物理堆峰值。

## 验证实际完成

- Debug 175 项、release 175 项通过；fmt、diff check、Clippy all-targets -D warnings、Rust 1.85 lib 和 release examples 通过。
- 两版重新编译的对照：公共 3,040 条、私有 34 条、专项 152 条，补丁、逆向和错误输出逐字节相同。专项新增 60 组独立对象迁移，涵盖重复值、覆盖目标、Add 先于 Remove、转义键和稳定数组祖先。
- 三个永久回归同时在基线和候选上通过：重复来源/顺序、祖先移除依赖、完整原始错误优先次序。
- JS 必需互通 1,073 例通过；上游自逆向失败和畸形文本头单独记录，不计为本库互通成功。
- 最终代码 nightly fuzz：Done 226880 runs in 61 second(s)，未崩溃；60 秒运行不能证明覆盖完整。
- 新增 Criterion 迁移 benchmark 实际运行通过，独立正向与逆向验证在计时外完成。

## 放弃的候选

第一版在任何匹配出现前扫描完整写入集合，首轮无匹配 Move 的 mixed800 慢约 10%，根 Add 组合慢约 7%–8%。该版本的测量被停止，没有纳入最终矩阵。最终版把证明移到第一个真实候选成功后；证据包保留早期记录和源码。

## 剩余成本与下一步

批处理针对已证明的独立对象成员；复杂依赖与数组位移仍可能二次重放。带 tests 的迁移还会在 rationalize/guard 成本计算中读取和序列化父快照，不能把本轮普通迁移的提速推广到这些成本。小输入只测了固定回归用例，完整解析流程没有在本轮优化。

优先继续研究父快照的成本复用；补丁合并是单独的功能扩展，本轮未加入。

复测：解压 evidence.zip，在 benchmark 目录执行 `python3 runner.py sources/baseline sources/candidate rerun fixtures`。该命令重新编译探针；需要锁文件对应的 Cargo 依赖。
