# Windows CI 修复、跨语言复测与性能采样（2026-10-06）

测量源码：[`b733c19`](https://github.com/majiayu000/palim/commit/b733c19286bfdd3153e377add563d1ba6ec806af)。相对 `8701b2f` 仅修改测试线程栈，生产生成路径相同。下列数据对应这份冻结源码；两个候选只在临时副本中实现，尚未进入生产代码或发布。

原始批次、各进程中位数、输入/源码/依赖 hash、完整 CPU 采样和候选源码覆盖保存在 [measurements.json](measurements.json)。未新增大型 evidence.zip。

## 1. Windows CI 已修复

`8701b2f` 的 Windows Debug tests 报告未命名线程栈溢出，旧 [CI](https://github.com/majiayu000/palim/actions/runs/37422013331) 的其它五个 job 成功。新 depth 回归测试在 128 KiB 线程中递归到 128 层；修改仅将测试栈设为 1 MiB，保留所有深度与宽容器断言。

修复提交的 [远端 CI](https://github.com/majiayu000/palim/actions/runs/37455708819) 六个 job 全部成功：Windows、macOS、Ubuntu 的 debug/release/fmt/Clippy，Rust 1.85 library check，JS 互通，60 秒 structured fuzz。相同生产代码在增大测试栈后转绿，支持平台 debug 栈帧差异导致该回归测试失败的判断；本轮未测量 Windows 单个栈帧大小。

## 2. 测量边界

- 固定版本：Rust json-patch 4.2.0、Go wI2L/jsondiff 0.7.1、fast-json-patch 3.1.1、jsondiffpatch 0.7.6。沿用 10-05 的锁文件；不声称它们是注册表当前最新版。
- Core：预解析 JSON → 公开生成 API → 销毁输出。Pipeline：相同 UTF-8 输入 → 解析两份文档 → 生成 → 序列化。Node 返回 JS string，Rust/Go 返回字节；本语料文本均为 ASCII。
- 编译完成后串行计时，3 次预热、至少 10 ms 校准（上限 2048 次）、9 批、3 个独立进程；顺序逐轮轮换。普通运行时 GC 不移出计时区间；Go 的一次显式 GC 位于 Core/Pipeline 之间。
- 67 个计划组合中完成 63 个、189 次进程测量，正确性失败为 0。40 种独立 RFC wire 经 Rust json-patch 正向应用，并用 Palim 根据基线生成逆补丁后独立还原；native 使用各自引擎 patch/reverse。
- 12 条预算跳过记录对应 4 个组合：Go optimized 的 100k 标量与百万 rotate 达到 **整个测量进程** 60 秒预算，后续轮次不再运行；JS native 的两个 20k/百万根数组按预声明 full-LCS 矩阵预算未执行。没有其延迟或 OOM 实测结论。
- Go optimized 不记录 Core：其 `CompareWithoutMarshal` 缺少 Rationalize 所需的目标序列化长度，不能用该入口冒充同功能的预解析测量。

## 3. RFC Pipeline（ms）

| 输入 | Palim plain | Palim opt | Rust json-patch | Go default | Go optimized | fast-json-patch |
|---|---:|---:|---:|---:|---:|---:|
| 小配置 | 0.0099 | 0.0108 | 0.0103 | 0.0167 | 0.0257 | 0.0034 |
| 100k 标量修改 | 75.2793 | 225.2651 | 88.8133 | 153.6348 | 预算超时 | 87.8517 |
| 5k 文件迁移 | 14.1455 | 44.0676 | 13.9747 | 75.3374 | 113.7361 | 23.1885 |
| rotate-2000 | 0.2693 | 0.2691 | 0.4937 | 1.7809 | 14.7242 | 0.3643 |
| rotate-1000000 | 191.9300 | 183.6011 | 263.8194 | 805.2559 | 预算超时 | 312.5023 |
| disjoint-20000 | 5.2044 | 25.9828 | 4.9789 | 16.1851 | 1390.0512 | 4.2310 |
| shuffle-2000 | 1.8270 | 2.2472 | 0.5327 | 1.5503 | 10.5191 | 0.2781 |

plain、opt 的能力与输出不同，不能将所有列解释为相同功能排名。Palim opt / Go optimized 都启用 factorize/rationalize，但候选策略与实际补丁不同。

| Core：plain vs Rust json-patch | Palim ms | json-patch ms | Palim / 对手补丁 B |
|---|---:|---:|---:|
| 小配置 | 0.0025 | 0.0029 | 53 / 53 |
| 100k 标量修改 | 14.6260 | 30.7065 | 5,978,896 / 5,978,896 |
| 5k 文件迁移 | 2.6984 | 2.6262 | 5,169,266 / 5,169,266 |
| 百万 rotate | 125.5182 | 116.7073 | 44 / 58,888,891 |
| disjoint-20000 | 2.3351 | 2.1411 | 1,148,891 / 1,148,891 |
| shuffle-2000 | 1.4786 | 0.2082 | 81,181 / 87,736 |

百万 rotate 的 Palim plain 三个进程中位数为 125.52、104.18、125.93 ms；同轮 json-patch 为 117.38、116.71、115.64 ms。此前 Pointer 报告中的 96.95 ms 是另一轮测量，不能据此宣称当前所有轮次都更快。本轮完整 pipeline 仍更快且输出小得多，生成阶段存在波动和剩余差距。

小配置和随机重排是 JS 的明确速度优势。Palim 的 migration opt pipeline 为 44.07 ms / 505,072 B，Go optimized 为 113.74 ms / 4,699,005 B；这组输入上时间和体积都更好，不能推广到任意输入。

## 4. Guard 与 native

| Guarded Pipeline | Palim ms / B | Go ms / B | fast-json-patch ms / B |
|---|---:|---:|---:|
| 100k 标量（plain + tests / invertible） | 148.34 / 11,657,786 | 212.10 / 11,657,786 | 107.50 / 11,657,786 |
| 5k 迁移（优化 + tests / optimized-invertible） | 70.31 / 5,201,949 | 139.28 / 9,398,362 | 35.41 / 10,043,156 |
| disjoint-20000（优化 + tests / optimized-invertible） | 49.40 / 680,072 | 1390.30 / 680,072 | 6.72 / 2,237,781 |

guard 验收不只检查 test 数量：标量容器新增无关键，三家都接受；迁移的无关 notes 漂移、disjoint 的额外数组尾项，Palim / Go 拒绝，JS 接受。这里只验证这些具体漂移，没有证明所有竞品具备相同保护范围，不能将更宽松 guard 的速度直接作为同合同优势。

| Native Pipeline | Palim ms | JS jsondiffpatch ms | 输出 B（相同） |
|---|---:|---:|---:|
| 小配置 | 0.0092 | 0.0059 | 28 |
| 100k 标量 | 105.5692 | 121.9060 | 2,767,797 |
| 5k 迁移 | 20.3427 | 21.2920 | 9,433,139 |
| rotate-2000 | 0.2503 | 39.1601 | 27 |
| disjoint-2000 | 1.4315 | 287.9724 | 111,790 |
| shuffle-2000 | 1.0224 | 89.7569 | 36,229 |

JS 在迁移 native Core 更快（3.01 vs 7.31 ms），完整 pipeline 差距很小且方向相反。大输入 native 跳过项不构成 Palim 横向胜出次数。

## 5. 采样定位与两个候选

release 探针分别重复 shuffle plain、隔离 factorize、隔离 rationalize，CLI `sample` 采样 8 秒，间隔名义 1 ms。factorize/rationalize 的输入 patch clone 放在阶段计时外，CPU 采样仍会捕获该调用方克隆。下列百分比是整个探针主线程的**叶节点样本占比**，不是剔除克隆后的精确阶段 CPU 占比；内联函数可能归入其它符号。

- **shuffle**：`walk` 367/6060、`increasing_matches` 98/6060；多种 BTreeMap 插入、memcmp/memmove、分配/释放也靠前。LIS 只有 82/2000，当前输出是 1918 个 move。不能仅凭 LIS 热点解释全部差距。
- **factorize**：`SipHash::write` 699/6086（11.5%），`format_escaped_str` 575/6086；调用树确认 scalar cache 使用 std RandomState，不只数组 interner。此前只核查数组 interner 的结论不完整。
- **rationalize**：`SipHash::write` 156/6075（2.6%）。主要回放分支 `rationalize_replacements → apply_json_patch_with_options → json_patch::apply_patches` 中有 `Value::pointer_mut` 与字符串 replace；这是依赖实现的 mutation 路径，不能因为本库已改 typed lookup 就声称全部 pointer 分配消失。memcmp、旧值比较、克隆、序列化与分配也明显。

采样下 factorize / rationalize 平均约 59.62 / 104.24 ms，仅用于诊断，不代替无 profiler 的批次中位数。

### Scratch A：RFC HashMap/HashSet 改用 foldhash 0.1.5

| opt Core | 当前 ms | 候选 ms | 时间减少 |
|---|---:|---:|---:|
| 100k 标量 | 164.4302 | 155.3936 | 5.5% |
| 5k 迁移 | 34.8881 | 29.9441 | 14.2% |
| disjoint-20000 | 24.2932 | 21.5322 | 11.4% |
| 小配置 | 0.00331 | 0.00332 | 无可用收益 |

三轮交替进程、同一 probe 边界，已测四组的 wire hash 完全一致且正逆应用通过。还未验证全部冻结记录、错误与 fuzz，不能据此认定生产可用。foldhash 已在间接依赖里，此候选会新增直接依赖并启用其 std convenience types；[作者说明](https://docs.rs/foldhash/0.1.5/foldhash/)其为非密码哈希且抗针对性碰撞能力有限。是否采用等待用户决定。

### Scratch B：强制 positional 的性能上限

仅在 plain、无过滤/自定义配对、等长 primitive 根数组上强制直接替换位置。
shuffle-2000：1.4964 → 0.1508 ms，81,181 → 87,736 B（+8.1%），1918 move → 1999 replace；三轮正逆验证通过。这不是完整的自适应算法：尚未计入收益判定成本，也不能用于 rotate，因为那会放弃其一条 move 的小补丁。

后续候选应只在移动收益低的输入退出，不新增公开模式；输出形状变化等待用户决定。native 的 LIS 移动质量和已有 callback/error 合同仍需要独立保留。

## 6. 测试覆盖与文本互通的更正

通用 `tests/properties.rs` 的确不生成 `_t`、`_0` 等协议键，但不能推断整个项目没有协议 fuzz：现有 `tests/protocol_properties.rs` 构造 move/delete/add/edit 数组协议，`fuzz/fuzz_targets/core.rs` 直接构造并变异协议索引、类型和文本 header；本轮远端 structured fuzz 成功。它们仍不是坏输入穷举。

远端 JS job 的 1073 组必需路径全部成功。JS 自逆成功 566 例中，strict 成功 471、零误差 fuzzy 成功 525、默认 fuzzy 成功 562；其余 4 个 header/operation 长度不符的格式明确报错。历史“95 个 inverse 被 strict 拒绝”对应 566−471，包含这 4 个畸形格式与 91 个 strict 未还原案例，不是当前默认 fuzzy 仍失败 95 例。冻结语料通过不保证任意 DMP matching 完全一致。完整 CI 日志已保存在 measurements.json。

## 7. 发布准备与当前优先级

crates.io 核查最新为 0.1.4，0.2.0 尚未发布。0.2.0 发布说明应明确 guard 输出字节变化；它不是普通性能补丁。本轮只准备版本和 changelog，不创建 tag 或发布。

发布准备的本机 `cargo fmt --check`、`git diff --check`、`cargo package --locked --allow-dirty` 均通过，打包后的 0.2.0 源码已由 Cargo 解包编译验证。复现脚本 quick 实际完成 8 个跨语言组合、4 次候选进程执行，正逆验证无失败；此 quick 结果不替代上方完整三轮计时。原始日志在 measurements.json。

1. Windows CI：已完成并远端全绿。
2. shuffle 的退出策略、foldhash 直接依赖：已提供可测量的原型与代价，待用户选择。
3. rationalize：主要剩余成本是回放、比较与分配，换哈希器只能解决一部分；保留深度、数字和错误边界。
4. 对比文档：使用本轮实测，保留 JS 小输入/随机重排、部分 Core 与 opt 分配成本反例；未核查真实下游采用，不宣称全面领先。

## 复现

需要 Python 3.12+、本机 Rust、Go、Node/npm、完整 Git 历史中的 `b733c19`，以及已跟踪的 10-05 evidence.zip。以下命令从冻结 commit 重建源码，在临时目录锁定依赖、先编译再串行计时，并输出新原始记录：

```sh
python3 results/ci-crosslang-20261006/run.py
python3 results/ci-crosslang-20261006/run.py --quick
```

quick 只验证小配置跨语言与小配置/shuffle 候选的重建和正逆应用，不复现完整表格。脚本还重建隔离 profile 二进制；在 macOS 上启动 `profile <shuffle|factorize|rationalize> <fixture>`，等待 READY 后使用 `/usr/bin/sample <pid> 8 1 -file <output>` 可重采样。冻结 profile 源码与原始样本均在 measurements.json，不改生产库 API。
