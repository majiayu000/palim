# 混合 JSON 生成与深度检查优化 — 2026-10-05

## 本轮修改

- 对象直接生成 RFC 6902 操作；遇到变化数组，只对该数组调用原有匹配器及导出器。移除了遇到一个数组就让整份文档退回 native Delta 的预扫描。数组匹配算法本身没有更换。
- 输入边界检查同时返回实际容器深度。当深度不超过 126 时，native leaf tuple 最多增加一层，能够证明 Delta 不超过既有 127 层限制；因此可以跳过重复的叶子/局部 Delta 深度遍历。较深输入继续检查，保持原有错误。
- 深度错误推迟到匹配遍历结束后返回，以保持原有后续 matcher callback 的执行。输入深度/冲突选项错误仍在遍历前返回。投影过滤仍使用原路径；未排序 Map 使用局部 native 路径。
- 新增混合对象、matcher 回调、深度错误顺序及随机混合 JSON 回归测试，常驻 benchmark 增加混合对象场景。未新增公开 API、依赖、版本或生产 unsafe。

## 原因与证据

**已验证：** 原来的 eligibility 遍历遇到改变数组后，整份对象仍构建可逆 Delta，再重新导出普通操作。新路径省去了对象部分的中间图；混合场景分配次数从 1,120,783 降到 304,096，额外存活分配峰值从 30,098,883 字节降到 11,131,643 字节。计数来自独立 allocator probe，不是 RSS。

**已验证：** 成员迁移消融实验中，单独去掉预扫描、输入深度检查、叶子深度检查或新增值规范化复制，都会改变成本。下表是不同二进制的测量，差值不能相加，也不能视为 profiler 的耗时占比。跳过验证/规范化的实验版本均未采用。

| 迁移 5,000 项的诊断版本 | Core ms |
|---|---:|
| baseline | 3.837511 |
| no-preflight | 3.353385 |
| no-input-depth | 3.501510 |
| no-leaf-depth | 3.439250 |
| clone-additions | 3.217302 |

生产版本保留 `json!(new)` 的数值规范化语义。递归按类型复制、先 clone 再递归规范化两个方案的三次结果均有重叠，没有稳定收益，因此未采用。剩余迁移差距包含值复制/规范化等工作；未通过 profiler 将每一项精确归因。

## 本轮前后比较

基线是本轮开始时的本地代码，含前几轮尚未发布的优化。每组 3 个独立进程，每进程 9 批；表格为进程中位数的中位数。所有配置顺序执行并交替先后顺序。Core 包含输出销毁；Pipeline 包含两份 JSON 解析、生成、紧凑 UTF-8 序列化及销毁。

| 场景 / 模式 | 基线 Core ms | 新版 Core ms | 速度比 | 基线 Pipeline ms | 新版 Pipeline ms |
|---|---:|---:|---:|---:|---:|
| mixed-array-last-100000 / plain | 86.151750 | 14.408959 | 5.98× | 146.537083 | 76.165667 |
| mixed-array-last-100000 / plain-guarded | 166.484916 | 96.319250 | 1.73× | 233.120750 | 162.707750 |
| mixed-array-last-100000 / optimized | 271.167917 | 196.792125 | 1.38× | 326.594417 | 254.339500 |
| file_migrations_move_copy_5000 / plain | 3.761771 | 3.221844 | 1.17× | 15.367041 | 14.244625 |
| file_migrations_move_copy_5000 / optimized | 38.531750 | 38.440083 | 1.00× | 47.106625 | 47.039500 |
| file_migrations_move_copy_5000 / guarded | 72.410167 | 72.582666 | 1.00× | 86.773792 | 86.706708 |
| scalar-guards-2000 / plain | 0.229476 | 0.223225 | 1.03× | 1.114445 | 1.116266 |
| scalar-guards-10000 / plain | 1.158898 | 1.128466 | 1.03× | 6.014187 | 6.065625 |
| scalar-guards-100000 / plain | 15.296334 | 14.413458 | 1.06× | 77.178292 | 74.708208 |
| scalar-guards-100000 / plain-guarded | 93.042584 | 93.753583 | 0.99× | 162.514042 | 161.787958 |
| scalar-guards-100000 / optimized | 168.740291 | 169.269125 | 1.00× | 227.202125 | 224.788667 |
| scalar-guards-100000 / native | 48.662750 | 48.823792 | 1.00× | 105.587208 | 104.612292 |
| disjoint-20000 / guarded | 49.818334 | 49.146417 | 1.01× | 52.251041 | 51.255208 |
| disjoint-20000 / optimized | 25.069625 | 25.382417 | 0.99× | 27.084083 | 27.219958 |
| rotate-2000 / plain | 0.228365 | 0.224027 | 1.02× | 0.383010 | 0.381079 |
| rotate-1000000 / plain | 189.959250 | 206.750084 | 0.92× | 283.604333 | 276.915542 |
| unchanged-100000 / plain-guarded | 0.586698 | 0.597546 | 0.98× | 11.107584 | 11.151791 |
| small-config-edit / plain | 0.002810 | 0.002495 | 1.13× | 0.010230 | 0.009925 |

`plain` 为普通操作；`plain-guarded` 只加入 test；`optimized` 启用 factorize/rationalize；`guarded` 三项均启用；`native` 为可逆 Delta。百万数组广泛测量的 Core 中位数有约 9% 差异，但每组范围明显重叠，因而增加五组交替复测，没有删掉原始结果。

| 百万数组旋转的追加复测 | Core ms | Pipeline ms |
|---|---:|---:|
| baseline | 213.946750 | 266.364958 |
| candidate | 219.951000 | 266.575708 |

追加复测和完整批次都保留在 measurements.json。该场景没有更换匹配算法，复测仍有较大进程间波动；不能据此宣称百万数组获得加速或证明所有输入无退化。

## 新测 Rust json-patch 对比

Rust json-patch 4.2.0 使用相同 serde_json 精度特性、输入 Values、边界及输出销毁，两个场景的紧凑 patch 字节完全相同。此处只比较普通对象操作，不代表数组移动、native Delta 或全部功能。

| 普通对象场景 | Palim Core ms | json-patch Core ms | Palim Pipeline ms | json-patch Pipeline ms |
|---|---:|---:|---:|---:|
| scalar-guards-100000 | 14.413458 | 31.157000 | 74.708208 | 88.791250 |
| file_migrations_move_copy_5000 | 3.221844 | 2.653687 | 14.244625 | 13.831334 |

Palim 在测得的十万标量字段场景领先；成员迁移仍落后，因此没有全面领先的结论。

## 分配计数

独立插桩二进制在已解析输入上生成一次；与计时分开。Calls 包含 allocation/reallocation；Peak 是额外请求的存活字节，不是 RSS。全部十次探针销毁输出后都恢复到原始存活字节。

| 场景 / 模式 | 版本 | Calls | Requested bytes | Peak extra live bytes |
|---|---|---:|---:|---:|
| scalar-guards-100000 / plain | baseline | 300,022 | 20,245,955 | 10,767,550 |
| scalar-guards-100000 / plain | candidate | 300,022 | 20,245,955 | 10,767,550 |
| scalar-guards-100000 / plain-guarded | baseline | 2,014,993 | 79,912,189 | 43,519,024 |
| scalar-guards-100000 / plain-guarded | candidate | 2,014,993 | 79,912,189 | 43,519,024 |
| file_migrations_move_copy_5000 / plain | baseline | 85,058 | 11,050,028 | 8,953,511 |
| file_migrations_move_copy_5000 / plain | candidate | 75,057 | 10,890,012 | 8,953,511 |
| disjoint-20000 / guarded | baseline | 916,673 | 56,443,616 | 19,771,135 |
| disjoint-20000 / guarded | candidate | 916,657 | 54,346,512 | 13,947,617 |
| mixed-array-last-100000 / plain | baseline | 1,120,783 | 50,751,446 | 30,098,883 |
| mixed-array-last-100000 / plain | candidate | 304,096 | 20,827,409 | 11,131,643 |

## 已完成验证

- `cargo fmt --check`。
- `cargo test --locked` / `cargo test --release --locked`：186 / 186 项通过。
- `cargo clippy --all-targets --locked -- -D warnings`。
- `cargo +1.85.0 check --lib --locked`。
- Release fixture_runner 构建及 JS 互通：1073 条，无失败。
- Nightly fuzz：211812 次，61 秒，无失败（配置预算 60 秒）。
- 3,305 条冻结差分记录字节一致，包括公开结果、内部优化决策、序列化成本和错误。
- 114 条正式计时进程记录、10 条旋转复测和 10 次分配探针；同场景前后 patch hash 一致。计时探针在计时外验证独立正向应用及 Palim 逆向恢复。
- 新的随机回归使用固定 seed，128 对混合 JSON × 8 种选项；比较 native 参考输出并检查正向及逆向。定向回归覆盖身份回调、移动选项、转义路径及深度错误后的后续回调。

## 限制与剩余方向

- 迁移中的新增值复制/规范化，以及 factorize/rationalize 和 guard shadow replay，仍有优化空间。本轮未改动这些算法。
- 未排序 Map 和配置投影过滤仍走 native 路径；本轮性能表使用默认排序 Map。
- 实际输出的大型数组 test 快照仍可能产生二次增长的字节体积；本轮没有更改守卫语义或协议。
- 单台 Apple M2 Max、warm、顺序测量；不涵盖 cold、并发、尾延迟、RSS 或跨平台。其他会话的修改已保留。本轮代码和结果尚未提交或发布。

## 可复查证据

批次、进程中位数、来源/输入指纹、patch hash、检查记录、分配计数和消融数据在 [measurements.json](measurements.json)。冻结源码、集成测试、探针、输入、oracle、日志及重建脚本在 [evidence.zip](evidence.zip)。
归档 22,528,163 字节，487 个 payload 文件；SHA-256 `41db5a2b38bb09a0ea38ac05e701bb4a5d4161c6b5d5e639d89ca3fa00c1e421`。已检查各文件 hash 与 ZIP CRC。

归档已解压到独立目录，重建两份探针并重放小输入：全部 payload hash、正向/逆向恢复及原始 patch hash 一致；命令 `python3 rebuild.py --replay-small`，exit 0。重建回执保存在 measurements.json。
