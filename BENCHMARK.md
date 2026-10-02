# Palim：Benchmark 与验证

本库已更名为 **Palim**（包名 `palim`）。以下历史测量与验证记录保留当时的名称 `jsondiffpatch-rs`，未因更名重新计时。

## 2026-10-02 文本逆补丁修复

保留 JS 原始逆补丁，修复默认 fuzzy 在重复 emoji 与 NUL/CRLF 重叠上下文中的
两个合法失败案例。优先搜索位移界限内的完整精确上下文；没有完整匹配时，
非零误差的长 hunk 只保留两侧各至多 4 个 UTF-16 单位的未修改上下文。
未提高误差阈值，零误差模式保留完整上下文，默认严格应用与格式校验保持原合同。

1,073 组必需互通路径全部通过。在 JS 自逆成功的 566 个案例中，
**562 个格式合法的逆补丁全部由默认 fuzzy 还原**；另外 4 个头部长度错误
继续返回 Error。严格应用仍为 471 个，零误差 fuzzy 仍为 525 个。
这是当前冻结语料的结果，不代表任意 DMP 模糊行为完全兼容。
[原始回归补丁](tests/fixtures/js-text-inverse.json)、
[互通统计](results/interop-summary.json)、
[本次验证记录](results/text-inverse-20261002.json)。本轮改动尚未发布。

## 2026-10-01 发布后补全

Palim **0.1.0 已发布**，发布包对应提交
`38eaa8dd05feaafa942e4c5ce2ad8996b133c33c`。
[该提交的远端 CI](https://github.com/majiayu000/palim/actions/runs/36841181994)
已实际完成 Linux、Windows、macOS 的 debug/release、fmt、Clippy，另有
Rust 1.85 库检查、JS 互通和 60 秒 fuzz；全部通过。
下方旧快照中的“尚未发布”“CI 尚未运行”是发布前状态。
本节的工作区改动与该已发布版本分开记录，不把旧 CI 当成新改动的验收。

### 重复 ID、补丁优化和 guards

同一 2000 项重复 ID 文档的定向探针，`factorize+rationalize+tests`
生成时间中位数 **4257.794 ms → 10.736 ms**，完整补丁仍为
**88,590 B，1 test + 1 replace**。前后均验证正向、标准逆向；这是
一轮预热后三轮定向测量，不是跨平台或业务加权排名。

带 guards 的移动补丁先尝试较大的父节点，避免已可整体替换的区域反复
重放完整补丁计算容器测试；其它补丁继续优先尝试更深的父节点。
这是启发式次序调整，可更早选择较粗的替换，不承诺任意输入都得到最小字节。

另对 16 个冻结 pipeline 负载逐一对照旧实现，补丁体积无增加，独立应用验证通过。
当前工作区的正式 pipeline 含 parse→diff→serialize，16 个输入各运行
3 个独立进程，共 **48 个测量与正确性验收**。
重复 ID 负载的进程中位数分别为 11.464、11.407、11.485 ms。
正式 pipeline 与上述预解析探针的边界不同，不能直接拼成前后倍率。
[正式原始样本和环境](results/standard-benchmark-pipeline-20261001T144404.173244Z/comparison.json)
保留了源码、runner、binary、fixture 与 Cargo.lock 哈希。

复现该组合：

```sh
cargo build --release --example standard_bench --locked
python3 tools/standard-benchmark.py --task pipeline --engine optimized-guarded
```

### JavaScript 自逆补丁的准确边界

1,073 组必需的正向、Rust 自逆及标准导出路径全部通过。
另把 JS 自己能还原的 **566** 个案例单独统计，避免把它们混入必需路径的结果。
严格应用还原 **471** 个；零误差、允许位移的显式 fuzzy 还原 **525** 个；
该轮默认 fuzzy 还原 **560** 个。剩余 4 个逆补丁的头部长度与操作不一致，
继续返回 Error；另有两个合法的重复文本案例，`text-22` 默认 fuzzy 结果
与原文不同，`text-99` 超出误差界限。这两个合法案例已于 10 月 2 日修复，
见上方记录。其它 507 个 JS 自逆失败另列。
[机器可读统计](results/interop-summary.json) 已更新为最新结果。

需要可靠 undo 时，导入 JS **正向 delta**，保存 Rust `reverse` 的结果，
或调用 `unpatch`。这条路径已在全部 1,073 组上验证。
直接应用外部 JS inverse 的 fuzzy 行为是独立的近似匹配合同，未宣称逐行为 DMP 兼容；
协议错误不能通过放宽模糊误差阈值绕过。

本次最终本地检查与范围记录见 [completion-20261001.json](results/completion-20261001.json)。

## 发布前验证快照

任务日期：2026-10-01。UTC 时间戳按机器实际值保留。Apple M2 Max、96 GiB，Rust 1.97.0、Node 24.14.0；正式测量串行执行，先验证输出，再计时。

## 实际完成

- Debug 与 release：各 126 个 integration tests、1 个 unit test、1 个 doctest，全部通过。
- 固定种子性质测试 28,160 个生成案例；另有 5,906 个唯一排列、4,032 个二元重复项排列、373 组冻结文档和 92 个公共 RFC 6902 用例。语料可能重叠，不相加称独立覆盖率。
- JS 互通 1,073 组，所有必需的 Rust/JS 正逆和 RFC 路径通过；JS 自己的 inverse 507 次失败单列，不代表 Rust 能应用所有 JS inverse。
- 独立检查：100,000 个导入数组 delta、100,000 个深度语义参考案例、13,208 个精确数值参考案例与 6 个预算边界、100 个长文本 Rust/JS 正逆案例通过。各自源码/二进制身份和范围见 results/review；这些不是额外的统一覆盖率。
- 最终 AddressSanitizer fuzz：427,750 次执行，115 秒时间预算，max_len 2048、seed 20261001、RSS 上限 1024 MiB，无失败样本。cov/ft 是 block/features 数，不是行覆盖率。
- fmt、Clippy（all targets，-D warnings）、Rust 1.85 库检查通过；Linux x86_64/aarch64、Windows MSVC、WASM 四个 target 库编译通过。没有跨 OS 运行/链接验收。三 OS CI 已写好，尚未在 GitHub 执行。
- Criterion 94 项；原生横向 36 个测量、4 个预算跳过；标准 pipeline 144、export 48、apply 18、inverse 6 个独立进程测量全部正确。export 另有 2 个 20k 重排仅正确性检查。

最终检查、哈希和未发布 package 状态见 [verification.json](results/verification.json)，机器可读前后对比见 [optimization-summary.json](results/optimization-summary.json)。

## 本轮优化前后

基线为上一轮已交付版本，冻结在 [baseline-before-optimization-round-2](results/baseline-before-optimization-round-2/verification.json)。以下 Criterion 采用同名、相同测量边界的 62 项旧负载；取 slope estimate，flat sampling 取 mean。新压力负载的修复前数据另外保存，不能冒充上一轮测过。

| Criterion 测量 | 之前 ms | 最终 ms | 之前 / 最终 |
|---|---:|---:|---:|
| core/diff/text | 0.186111 | 0.005091 | 36.56× |
| core/diff/rotate-20000 | 3.291492 | 3.603317 | 0.91× |
| core/diff/keyed-20000 | 6.849011 | 6.246190 | 1.10× |
| standard/export-precomputed/halfswap-500 | 0.344592 | 0.183958 | 1.87× |
| standard/optimized/disjoint-2000 | 14.067998 | 4.655647 | 3.02× |
| standard/optimized/keyed-2000 | 3.475472 | 1.642909 | 2.12× |
| compare/unordered-multiset-2000 | 0.982921 | 0.952573 | 1.03× |
| compare/absolute-tolerance-2000 | 0.766179 | 0.661406 | 1.16× |
| merge/diff-settings-1000 | 0.185319 | 0.187365 | 0.99× |
| merge/apply-settings-1000 | 0.156274 | 0.156974 | 1.00× |
| core/patch/keyed-2000 | 0.374060 | 0.506187 | 0.74× |
| owned/patch-preowned/keyed-2000 | 0.026475 | 0.025364 | 1.04× |

全部区间、20 个 samples 和 benchmark 哈希见 [criterion.json](results/criterion.json) 与 [criterion-environment.json](results/criterion-environment.json)。20 samples、100 ms warmup、300 ms target、5000 bootstrap resamples，慢负载可自动延长。单机噪声和不同算法成本仍存在，表中包含退化，不能归因为某一个修改。owned baseline clone 在计时外准备，收益只适用于调用者已有可消费 Value；计入 clone 后不能据此声称端到端提升。

### 压力测试发现的退化

- 5000 行文本三处 Unicode 修改：Criterion 22306.999 ms → 1.624 ms。[修复前证据](results/diagnostic-three-edit/criterion.json)。[Imara 上游修复](https://github.com/pascalkuthe/imara-diff/commit/f02e46a737c43c0d9c65a42e5e08046bd10fd739) 修正重复 token 预处理扫描范围；本库采用有界行组与小区间细化，无 Git/vendor 依赖。较大变化可产生更粗但精确可逆的 replacement，不保证最小文本 patch；fuzzy 和数组 fallback 不受这一 token 上限覆盖。
- 2000 项重复 ID，optimized 标准 pipeline：1456.175 ms → 24.375 ms。[修复前进程样本](results/standard-benchmark-pipeline-20260930T210634.482409Z/comparison.json)。copy 搜索的空路径成本下界曾误判收益，反复全树扫描却没有生成 copy；按有效路径字节预算剪枝后的最终样本与语义验证分别保留。

## 原生 delta：Rust 与 JS

双方 parse 两份 JSON → native diff → serialize 完整可逆 delta，相同 object_hash 与 UTF-16 阈值。每个进程 20 次预热、5 个自适应计时批次，表中是进程内批次中位数，不是服务 P99。

| 场景 | Rust pipeline ms | JS pipeline ms | Rust / JS delta 字节 |
|---|---:|---:|---:|
| small-config-edit | 0.01480 | 0.00675 | 28 / 28 |
| rotate-2000 | 0.45894 | 38.81808 | 27 / 27 |
| keyed-rotate-edit-2000 | 2.62967 | 80.96996 | 74 / 74 |
| crates-metadata-rotate-edit-100 | 0.82040 | 0.98588 | 141 / 141 |
| long-text-ascii | 0.17338 | 0.32058 | 56 / 83 |
| long-text-unicode | 0.36115 | 0.31511 | 56 / 151 |

16 对可比较输入中 Rust 快 10 对。这是负载计数，不是业务加权排名；小对象及部分解析/应用边界仍可能慢于 JS。纯 diff、patch、reverse 和原始批次见 [comparison.json](results/comparison.json)。JS 20k rotate/reverse/disjoint/sparse 四项按事先内存预算跳过，未实测失败。

## 标准 JSON Patch：完整 pipeline

三方均 parse 相同左右 JSON bytes → typed RFC Patch → serialize。json-patch 按位置比较数组；plain 关闭 factorize/rationalize/tests；optimized 开前两项、不开 tests。公开 API 的合同检查计入本库；json-patch 没有额外 Value 转换。

每个输入/引擎独立启动 3 个进程，轮换顺序；每进程 20 次预热、至少 25 ms 校准（上限 16384 iterations）、10 个批次。下表取进程中位数的中位数。旧 6 对输入的 left/right 全部一致，新增 tasks 元数据导致完整 fixture 哈希不同，已另外保存规范输入对哈希。

| 输入 | json-patch ms / B / 条数 | plain ms / B / 条数 | optimized ms / B / 条数 |
|---|---|---|---|
| small-config-edit | 0.01039 / 53 / 1 | 0.01512 / 53 / 1 | 0.02377 / 53 / 1 |
| rotate-2000 | 0.47633 / 112,891 / 2,000 | 0.47467 / 42 / 1 | 0.49206 / 42 / 1 |
| disjoint-2000 | 0.48077 / 112,891 / 2,000 | 2.59958 / 165,781 / 4,000 | 4.53997 / 34,038 / 1 |
| keyed-rotate-edit-2000 | 3.51909 / 479,232 / 8,000 | 2.64793 / 116 / 2 | 4.48561 / 116 / 2 |
| long-text-ascii | 0.24905 / 180,044 / 1 | 0.25778 / 180,044 / 1 | 0.54820 / 180,044 / 1 |
| halfswap-500 | 0.11813 / 27,891 / 500 | 0.28973 / 10,001 / 250 | 0.36741 / 8,538 / 1 |
| unicode-three-edits | 0.44713 / 230,060 / 1 | 2.07585 / 230,060 / 1 | 2.45464 / 230,060 / 1 |
| duplicates-high-2000 | 0.48494 / 82,951 / 2,000 | 2.42414 / 1,500 / 42 | 2.61197 / 1,500 / 42 |
| ambiguous-ids-2000 | 1.68743 / 190,039 / 4,000 | 3.71797 / 100,688 / 2,021 | 24.37534 / 44,296 / 1 |

移动输出可显著变小，optimized 也可能牺牲生成时间换体积。全异、长文本、多重复身份仍有位置式 json-patch 更快的反例。halfswap-500 是字符串，plain 250 moves，optimized 是根 replace；不能把 replace 当成保留移动语义，也不能混用旧整数半区交换字节数。

## 标准导出、应用和逆补丁

以下各有独立边界：export 已有 native Delta → RFC export → serialize；apply 不含解析，包含不可变 baseline copy、相同 typed operations 的执行和结果序列化；inverse 为 baseline + 原操作 → inverse → serialize。不能跨边界比较排名。

| 场景 / 任务 | 本库 ms | json-patch ms |
|---|---:|---:|
| halfswap-500 / export | 0.19792 | 无对应 native export |
| shuffle-2000 / export | 1.76549 | 无对应 native export |
| reverse-2000 / export | 1.66147 | 无对应 native export |
| ambiguous-ids-2000 / export | 2.33524 | 无对应 native export |
| batch-replace-2000 / apply | 1.40689 | 1.45616 |
| batch-guarded-4000 / apply | 2.09799 | 2.12960 |
| batch-inverse-2000 / apply | 1.43022 | 1.44710 |
| batch-replace-2000 / inverse | 2.72238 | 无相同 API 测量 |
| batch-guarded-4000 / inverse | 3.38634 | 无相同 API 测量 |

所有正式标准数据、每进程原始批次、fixture/source/binary hashes、RSS 与环境：

- [pipeline](results/standard-benchmark-pipeline-20260930T214341.446185Z/comparison.json)：144 个测量；144 个正确性验收。
- [export](results/standard-benchmark-export-20260930T214448.844863Z/comparison.json)：48 个测量；50 个正确性验收。
- [apply](results/standard-benchmark-apply-20260930T214511.142280Z/comparison.json)：18 个测量；18 个正确性验收。
- [inverse](results/standard-benchmark-inverse-20260930T214521.102315Z/comparison.json)：6 个测量；6 个正确性验收。

### 带 guards 的优化仍有慢项

独立定向探针中，同一个重复 ID 负载：普通 guarded 约 12.3 ms，factorize+rationalize+tests 约 4.2 秒。瓶颈是约 2000 个父候选反复模拟整份补丁的 guards；copy 修复没有消除它。这里是保存的局部 profile，未纳入上述不带 tests 的正式 optimized pipeline。为避免引入每候选/每操作双重状态与依赖分析，本轮保留真实代价，未跳过认证或关闭 guards。该组合适合优先缩小带测试补丁、容忍生成开销的场景，不应视作低延迟默认选择。[探针及原始输出](results/review/rfc-copy/after.txt)。

## 内存方案探索

采用已知目标复用、typed operations、counting writer、按需 shadow/index；没有新增借用 delta 或 parser API。五个样本、113 个计量 phase 的分配结果见 [allocation/REPORT.md](results/review/allocation/REPORT.md)。gross requested bytes 是 alloc 请求与 realloc 新尺寸的累加，既不是活跃内存，也不是 RSS。探针计时与正式性能测量分开。

| 默认 RFC 生成 | gross 请求 B，探索前 → 探索后 |
|---|---:|
| 2000 项身份移动与编辑 | 22,740,151 → 4,653,270 |
| 1 MiB 未变化子树中的字段修改 | 14,694,229 → 2,728 |
| 长 ASCII 文本 | 3,910,453 → 701,998 |
| 长 Unicode 文本 | 4,193,797 → 681,998 |

这些是已保存探索快照的总收益，探索后的快照早于最终文本/RFC修复；原始前二进制已重放，但没有捕获它精确的编译源哈希，因此不称最终版本精确归因。原生已有 owned API 和 serde_json::to_writer 可复用 buffer。RawValue 适合不透明 JSON 转发，单根借用原型不能代表完整结构 diff。全局数值缓存使唯一小数和整数增加分配，未采用。

## 复现与限制

```sh
cargo test --locked
cargo test --release --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
cargo +1.85.0 check --lib --locked
cargo bench --bench core --locked -- --noplot
npm ci --prefix tools --ignore-scripts --no-audit --no-fund
cargo build --release --example fixture_runner --example standard_bench --locked
node tools/interop.mjs
python3 tools/benchmark.py
python3 tools/standard-benchmark.py --task pipeline
python3 tools/standard-benchmark.py --task export
python3 tools/standard-benchmark.py --task apply
python3 tools/standard-benchmark.py --task inverse
cargo +nightly fuzz run core -- -max_total_time=115 -max_len=2048 -rss_limit_mb=1024 -seed=20261001
cargo package --allow-dirty --locked
```

完整生成器、种子与首次 512 MiB fuzz 限额导致的 OOM 诊断也保留在 results/fuzz：当时空输入可成功重放，活跃堆约 42 MiB，RSS 包含 libFuzzer 特征和 ASan quarantine；最终只增加测试运行器 RSS 限额，未减少断言或关闭 sanitizer。

单机冻结输入，无真实业务加权、跨生态统一运行环境或已核查下游采用。RSS 是整个进程峰值，含运行时、验证、预热、解析。大重复数组仍使用 Imara Histogram fallback，精确 LCS 有候选上限；未保证全部路径线性、最小字节、任意 JS inverse / DMP fuzzy 完全兼容或全球最快。历史首轮数据另见 [baseline-20260930](results/baseline-20260930/verification.json)。

## 小对象与普通 RFC 数组路径（2026-10-01 UTC）

本节比较已发布的 0.1.0（`38eaa8d`）与性能分支。边界仍为 parse 两份相同 bytes → typed RFC diff → serialize Patch。每个版本、引擎、输入运行 3 个独立进程，每进程保留 10 个原始批次；交错版本顺序，输出在计时前通过独立 json-patch 应用。共 66 个正确且完成的进程测量，不与历史测量累加为覆盖率。

| 输入 / 模式 | 0.1.0 ms | 性能分支 ms | 0.1.0 / 分支 | 补丁条数，之前 → 之后 |
|---|---:|---:|---:|---:|
| small-config-edit / plain | 0.016195 | 0.011235 | 1.44× | 1 → 1 |
| small-config-edit / optimized | 0.025551 | 0.020380 | 1.25× | 1 → 1 |
| disjoint-2000 / plain | 2.627504 | 0.512221 | 5.13× | 4,000 → 2,000 |
| disjoint-2000 / optimized | 4.540760 | 4.472508 | 1.02× | 1 → 1 |
| rotate-2000 / plain | 0.512542 | 0.480542 | 1.07× | 1 → 1 |
| reverse-2000 / plain | 2.195029 | 2.033635 | 1.08× | 1,999 → 1,999 |
| duplicates-high-2000 / plain | 2.633945 | 2.624807 | 1.00× | 42 → 42 |
| prepend-2000 / plain | 0.547572 | 0.544575 | 1.01× | 1 → 1 |
| unchanged-2000 / plain | 0.232994 | 0.231296 | 1.01× | 0 → 0 |

全异 plain 补丁从 165,781 B 缩至 112,891 B。相同轮次 typed json-patch：小配置 0.011153 ms，全异数组 0.473536 ms；小配置接近，全异数组本库仍约慢 8%。optimized 全异数组依旧以生成成本换取 34,038 B 的单次父替换；本轮没有消除该模式的开销。

改动范围：

- 深度检查仅把容器放入工作队列，保持既有容器深度和错误合同。
- 无 node filter 时，未变化的 number/bool/null 属性跳过 JSON Pointer 分配；property filter 仍先调用。
- plain 全异数组按位置 Replace 重叠项、倒序 Remove 多余尾项、顺序 Add 新尾项。根级标量数组在无过滤器、自定义匹配时直接生成 RFC 操作，省去可逆 delta 临时构造。
- factorize 或 tests 开启时保留原数组生成路径，以保留后续 Copy 机会和数组容器 guards。公共 native diff 的 delta 格式保持；公开 Delta 导出仍先严格验证 baseline。

分支独立完成 134 项测试、Clippy、fmt 与 Rust 1.85 库检查。另在临时目录组合主工作区的 guards 修改，138 项测试及 Clippy 通过；这份组合验证不表示主工作区已经合并或发布。

[全部进程与原始批次](results/diff-performance-direct-20261001/summary.json)、[环境及源/二进制/输入哈希](results/diff-performance-direct-20261001/environment.json)、[实际检查输出及组合快照](results/diff-performance-direct-20261001/verification.json)。单机短时测量仍有波动；小幅变化不能证明普遍性能提升，表中 RFC 数据不代表 native Rust/JS 性能排名。

复现时为两个 worktree 使用各自的 target 目录，避免相同包名的编译缓存混用。分别构建 0.1.0 和候选分支的 `standard_bench` 后运行：

```sh
python3 results/diff-performance-direct-20261001/run.py \
  /path/to/candidate /path/to/published-worktree \
  /path/to/published-worktree/target/release/examples/standard_bench \
  /path/to/new-results-directory
```

## 已落地的性能修复（2026-10-02）

本轮基于 `db9339d`，整合上节 plain 数组改动，并修复 RFC 优化和 custom matcher 的额外工作。以下为最终代码的重新测量，不能与前文不同计时边界的数据混合。

### 依赖选择

采用已发布的 [gix-imara-diff 0.2.5](https://docs.rs/gix-imara-diff/0.2.5/gix_imara_diff/)，通过 Cargo package alias 保留内部导入名称。其发布源码包含 `pos.saturating_sub(WINDOW_SIZE)` 修正；下载 archive、registry checksum 和 lockfile 一致。此依赖随 crate 正常解析，生产 manifest 没有 path/git override 或 `[patch]`。

选择约束是保留 Rust 1.85、现有序列算法和公开 API，并让下游自动得到修正。0.3.0 要求 Rust 1.88；`immigrant-imara-diff 0.2.1` 发布源码仍有窗口问题；本地 patch 无法通过依赖包传给消费者。已发布的 Gitoxide 实现满足约束，因而不引入私有引擎副本。风险是维护分支还包含其他算法修正，验证使用原生/RFC往返、现有质量测试、JS互通、fuzz和解包后的消费者。

### 最终代码对照

输入已解析；计时包括生成和返回值析构，不包括解析、序列化或验证。两个版本使用独立 target；每项3个进程，每进程3次预热、至少5ms校准（上限2048次）、8批样本；版本顺序轮换。下表为进程中位数的中位数。

| 场景 | API/选项 | db9339d ms | 修复后 ms | 倍率 |
|---|---|---:|---:|---:|
| 周期8、20,000项旋转 | native diff | 187.683 | 5.220 | 35.95× |
| 周期8、40,000项旋转 | native diff | 751.949 | 10.310 | 72.93× |
| 小配置编辑 | 默认 RFC | 0.011610 | 0.004015 | 2.89× |
| 大片内容未变、改一个字段 | 默认 RFC | 0.151865 | 0.007799 | 19.47× |
| 根部大文本增加一个字符 | 默认 RFC | 0.533874 | 0.171746 | 3.11× |
| 空源、8,000项目标 | custom native | 24.847 | 2.209 | 11.25× |
| 单项源、8,000项不匹配目标 | custom native | 25.202 | 2.271 | 11.10× |
| 全异2,000项数组 | plain RFC | 2.260 | 0.219 | 10.31× |
| 全异2,000项数组 | 默认 RFC | 4.337 | 4.362 | 0.99× |
| 唯一项旋转2,000项 | plain RFC | 0.326 | 0.332 | 0.98× |
| 重复身份2,000项 | 默认 RFC | 23.731 | 24.245 | 0.98× |

66个进程测量均先验证应用与逆向/撤销。除了 plain 全异数组由4,000个 Remove/Add 改为2,000个 Replace，其他10种输入/选项的规范输出哈希均相同。控制负载没有显著改善，两个场景约2%的增加也保留在表中；单机测量不足以判断普遍排名。重复身份数组的父节点压缩成本，以及默认全异数组先构造大补丁的成本，本轮仍存在。

RFC 改动为：单操作直接进行有预算的 Copy 搜索；没有候选父节点时返回；不能比当前补丁更小的候选提前停止字节计数。保留数学数值相等、Copy对数组的插入语义、guard成本及真实序列化错误。未全局改成宽优先压缩，避免已观测到的补丁体积增加。

Custom matcher 保留原候选枚举、BFS和配对顺序，复用访问代数、前驱和队列，并省去此分支无用的 interning。任意自定义 matcher 的候选判断仍可能需要 `n×m` 次调用；本轮收益来自去除额外的二次初始化。

### 完成的检查

- debug/release各146项测试、fmt、Clippy `-D warnings` 通过；新增回归覆盖重复数组、奇数长度、回调顺序、歧义配对、root Copy、array Replace与字节预算边界/真实错误。
- Rust 1.85 库检查通过；`cargo package --allow-dirty --locked` 解包验证通过。另一个无预置锁文件的 Rust 1.85 消费者使用解包产物运行20,000项重复数组往返成功，依赖从 registry解析。
- JS互通1,073个用例，Rust delta前向、Rust inverse反向、Rust RFC前向与JS delta经Rust往返全部通过。上游JS inverse限制继续按已有分类报告，不能解释为完全兼容全部JS inverse。
- 固定seed、max_len2048的结构化fuzz运行61秒、399,241次，无崩溃。这是短时检查，不是无缺陷证明。
- 独立只读审查没有阻断问题；本地未重新执行跨平台CI。

[全部进程和批次](results/performance-fixes-20261002/summary.json)、[环境/输入/源码/二进制哈希](results/performance-fixes-20261002/environment.json)、[检查输出](results/performance-fixes-checks-20261002/verification.json)、[消费者](results/performance-fixes-checks-20261002/package-consumer.json)、[fuzz记录](results/performance-fixes-checks-20261002/fuzz.json)。初次Clippy的测试范围写法警告和修正后的成功输出均保留。

复现：

```sh
python3 tools/performance-fixes.py \
  /path/to/db9339d-worktree /path/to/fixed-worktree \
  /path/to/new-results-directory
```

本轮完成代码修复和本地验证，没有重新发布crate。以上收益不代表包含解析和序列化的完整业务流程，也不代表所有输入都最快。
