# 文本与补丁生成性能：2026-10-03

## 结果与范围

本轮相对 `afe18686171131963f1ceb5237014f1590d693ed`，修改 `src/diff.rs`、`src/rfc.rs`、`src/text.rs` 和对应回归测试，增加可复现 benchmark runner。没有更改 Cargo、依赖、公开 API、选项、数值精度或父候选次序。实现位于隔离 worktree，计时时尚未提交；候选身份以冻结源码、锁文件和二进制哈希为准，不能把其当时的 HEAD 当成候选代码身份。

**已证：** 400 个独立对象的普通压缩生成从 191.804 降至 8.862 ms，约 21.64×；Unicode 三处编辑从 1.270 降至 0.482 ms，约 2.64×。所有12种输入/模式的完整输出与基线逐字节相同，独立正向和逆向应用成功。小配置也有变慢样本，保留如下。

**已证：** 本轮同一串行竞品测量中，Unicode 三处编辑完整 native 流程 Palim 0.790 ms、JS 1.242 ms，均311 B；小配置完整流程仍是 JS 更快。没有任意输入最快、全部功能领先或全球最小补丁的证明。本轮没有新增功能，版本仍0.1.0，尚未重新发布。

## 采用的实现与依据

### RFC 候选压缩

- 用借用迭代器表达候选，只有接受后才转移原操作所有权，避免克隆整份补丁。
- guard 字节成本本来已执行完整候选；直接复用最终 scratch 文档确认目标，省掉第二遍克隆和重放。
- 普通候选首先惰性验证原补丁到达目标。仅当外部操作不写入祖先、不跨边界读取/移动，且上级数组没有会移动边界的结构操作时，用独立子树替换证明省去重复模拟。根替换、单操作及所有未证明独立的候选继续真实应用验证。目标深度在原公开边界已检查，保留原逐操作错误处理。
- 最终操作数缩减时，末尾仅执行一次 `shrink_to_fit`，避免 in-place collect 保留原数千条操作容量。无缩减的小配置和旋转不执行收缩。

**已核查源码：** [Go jsondiff v0.7.1](https://github.com/wI2L/jsondiff/blob/v0.7.1/differ.go) 以局部生成序列和父替换的序列化大小比较，提供局部独立性的先例；其数组匹配与本库不同，未照搬免验证替换。[json-patch 4.2.0](https://docs.rs/json-patch/4.2.0/src/json_patch/lib.rs.html) 的借用操作执行方式可复用思路。本库继续使用原 source-depth 和逐操作 helper。拒绝重新排父候选、全局重放缓存、新 DOM/backend 与公开策略配置。

### 文本 token 与坐标

- 行 token 使用原文的借用 `&str`，替代 `&[char]` key；相等关系和首次 interning 顺序相同，因此 token IDs 不变。
- 边界仅保存 byte offset。有序 hunk 用每侧两个 usize 游标累计字符坐标，继续交给原字符细化和 UTF-16 渲染。
- 每8个字节计算精确 LF mask；采用 `from_le_bytes` 与逐低位枚举保证字节顺序，末尾0–7字节逐个扫描。每 lane 的低7位加0x7f不会向邻 lane 进位，OR原差值再取反恰好识别 LF。没有 unsafe、密度阈值或依赖。
- 字符向量、行分组、序列容量上限及错误路径保持原合同。LF 不会出现在 UTF-8 多字节内部，所以 byte+1 必为合法边界。

**已核查源码：** 锁定的 [gix-imara-diff 0.2.5 HunkIter](https://docs.rs/gix-imara-diff/0.2.5/gix_imara_diff/struct.HunkIter.html) 文档与发布实现都保证两侧 hunk 有序且不重叠；游标不会回退。独立审查与上游源码 hash 保存于 evidence.zip。

### 原子替换

已验证有效 JSON 的替换 delta 直接克隆 Value，不再通过 `json!` 重新 Serialize。新增回归包含20种公开解析/构造的数值及混合 payload，保持原 Value 与 wire 表示、正逆及 RFC 应用。未调整 arbitrary_precision 或 float_roundtrip。

## 最终内部对照

当前 macOS arm64，Rust1.97.0。两个版本分别冻结源码并使用独立 target；计时二进制为默认分配器，分配计数使用另一二进制。没有把计数开销计入速度排名。计时包括已解析 Value 的生成和结果析构，不包含解析、序列化和验证；先用独立 json-patch/原生正逆验证，再开始计时。

每种输入/模式：3次预热，至少10ms校准（上限2048次），9个批次，3个独立进程/版本，轮换版本顺序；表取进程中位数的中位数。12种输入/模式×2版本×2测量类型×3进程=144。所有原始批次、进程中位数、输入/源码/锁/binary hashes 在 [measurements.json](measurements.json)，归档中保留构建快照。共享机器的短时结果不等于服务P99。

| 场景 / 模式 | afe1868 ms | 本轮 ms | 倍率 | 分配或重分配次数 | 峰值额外 live B |
|---|---:|---:|---:|---:|---:|
| small-config-edit / native | 0.002760 | 0.002837 | 0.97× | 24 → 20 | 1359 → 1359 |
| small-config-edit / optimized | 0.004121 | 0.004483 | 0.92× | 52 → 48 | 1952 → 1952 |
| rotate-2000 / native | 0.332991 | 0.311426 | 1.07× | 8044 → 8044 | 364104 → 364104 |
| disjoint-2000 / optimized | 2.733250 | 2.623141 | 1.04× | 42410 → 40409 | 1263230 → 1263230 |
| ambiguous-ids-2000 / optimized | 8.317167 | 7.863541 | 1.06× | 173745 → 155744 | 7961701 → 6609187 |
| many-accepted-100 / optimized | 12.425458 | 1.393167 | 8.92× | 325838 → 19689 | 482813 → 432543 |
| many-accepted-100 / guarded | 25.489667 | 14.251042 | 1.79× | 616717 → 306244 | 484461 → 472805 |
| many-accepted-400 / optimized | 191.804459 | 8.861917 | 21.64× | 4964606 → 78385 | 1930405 → 1729763 |
| many-accepted-400 / guarded | 371.839167 | 235.403667 | 1.58× | 9314339 → 4410888 | 1936853 → 1891121 |
| unicode-three-edits / native | 1.270484 | 0.481927 | 2.64× | 127 → 127 | 1956654 → 1956654 |
| unicode-disjoint-24000 / native | 2.672937 | 2.550312 | 1.05× | 100 → 100 | 2972844 → 2972844 |
| single-text-edit / native | 0.026713 | 0.027102 | 0.99× | 23 → 23 | 594 → 594 |

峰值额外 live B 是 diff 范围内，同时存活的成功分配请求字节相对开始点的增量；不包含预先持有的输入，不是 RSS 或分配器物理容量。requested_bytes 则是 alloc/realloc 新请求尺寸累加，与活跃峰值不同。所有进程返回后 live 增量为0。400对象 guarded 请求累积字节从537,809,310降至212,697,524 B，峰值仅下降约2.4%；不能混用两者。

小配置 native +2.8%、optimized +8.8%，单文本追加 +1.5%：这一轮的小幅回退保留。其它阶段的小配置 optimized 有更快和更慢样本，未据此归因为某一行代码或证明普遍改善。两类文本的分配次数、请求总量和峰值均与基线相同。

## 新鲜竞品对照

固定版本：jsondiffpatch JS0.7.6、Rust json-patch4.2.0、Go jsondiffv0.7.1。4个RFC输入×6引擎×3进程=72，4个native输入×3引擎×3进程=36，共108个串行完成的进程。与内部已解析测量分别报告，不能拼接倍率。输入、各引擎完整批次和生成补丁身份均保存。

### Native：parse 两份 JSON → diff → serialize 可逆 delta

JS 两模式都设置相同 ID callback；default 是 position 选项未指定，no-position 显式关闭它。Rust 同样设置 ID callback。这不是未配置的 JS `create()` 比较。先确认各方正向、逆向；本次4个输入的 JS inverse 均成功。fixture_runner/JS 都含各自正常运行时和分配回收边界，不把它解释为跨语言统一分配器排名。

| 场景 | Palim ms | JS default ms | JS no-position ms | Rust / JS default / JS no-position B |
|---|---:|---:|---:|---:|
| small-config-edit | 0.010077 | 0.006711 | 0.007587 | 28 / 28 / 28 |
| rotate-2000 | 0.606269 | 44.435167 | 42.895750 | 27 / 27 / 27 |
| disjoint-2000 | 1.597528 | 342.357958 | 307.038458 | 111790 / 111790 / 111790 |
| unicode-three-edits | 0.789628 | 1.242061 | 1.301150 | 311 / 311 / 311 |

小配置纯 diff：Palim 0.002722 ms、JS default 0.004903 ms；加入解析和序列化后次序相反。这是具体负载的边界差异，不代表所有 JSON 解析占比相同。

### RFC：parse 两份 JSON → typed JSON Patch → serialize

plain 关闭 factorize/rationalize/tests，optimized 开前两项。Go optimized 同样开启 factorize/rationalize；其默认位置匹配与本库身份匹配并不相同。表格每格为 ms / B / 操作数。其余 Go default、optimized-lcs 数据保留于 JSON，无未完成或超时项。

| 场景 | json-patch | Palim plain | Palim optimized | Go optimized |
|---|---|---|---|---|
| small-config-edit | 0.011752 / 53 / 1 | 0.012189 / 53 / 1 | 0.014098 / 53 / 1 | 0.037068 / 53 / 1 |
| disjoint-2000 | 0.555495 / 112891 / 2000 | 0.534833 / 112891 / 2000 | 3.227155 / 34038 / 1 | 16.970458 / 34038 / 1 |
| long-text-ascii | 0.285323 / 180044 / 1 | 0.285517 / 180044 / 1 | 0.359755 / 180044 / 1 | 3.536075 / 180044 / 1 |
| unicode-three-edits | 0.501264 / 230060 / 1 | 0.511117 / 230060 / 1 | 0.590209 / 230060 / 1 | 5.166568 / 230060 / 1 |

Go和Palim optimized 全异数组均输出34038 B的根替换；位置式 json-patch生成2000条替换，速度更快、体积更大。不能把其中任一指标单独当成全部领先。RFC 文本替换整串，与 native 的311 B文本 patch不是同一功能边界。

## 从回退中修正的方案

这些阶段数据只用于选择实现，不混入最终表：

1. 双坐标 tuple 边界使 Unicode3 峰值多32768 B；改为单 byte边界和4个栈游标，最终内存回退消除。
2. IntoIter/filter_map/collect 保留了2048条操作容量。独立容量/分配探针证明，400对象 guarded 多出的峰值来自此中间补丁；末尾一次 shrink使其下降131008 B，最终低于基线45732 B。
3. `match_indices` 在24k重复短行场景约慢12%。profile显示两侧边界搜索约483–488μs，整窗重复字符计数仅约14.5μs；端点复用只改善约0.6%，拒绝增加该分支。
4. scalar byte扫描解决短行，却降低长行收益。安全8byte mask局部对照将Unicode3 0.624→0.468ms，短行约持平。最终整树重新构建144/108进程，未复用原型二进制或冒用其时间。

阶段源、完整输出、原始样本、独立参考和选择理由都保留在 [evidence.zip](evidence.zip)。

## 已完成检查

- 最终 debug/release各163项；fmt、Clippy all-targets `-D warnings`、Rust1.85库检查通过。
- RFC 373对×8选项=2984：完整 wire逐字节等于afe1868；正向/逆向验证通过，绑定最终RFC模块hash。
- 文本313冻结案例及5个正式fixture：native/inverse/RFC完整输出等价。新增mask unit覆盖8 lanes×128 ASCII、LF/VT、UTF8混排、0–7前后对齐和不同group。
- JS互通1073个必需路径全通过。上游自身inverse507次失败单列；566个自身成功中，562个合法inverse由默认fuzzy还原，4个格式错误继续返回Error。没有扩大模糊合同。
- AddressSanitizer结构化fuzz，seed20261001、max_len2048、RSS上限1024MiB：61秒、299738次执行，无失败。命令墙钟包括构建共98.5秒；执行次数不是行覆盖率。
- cargo package实际打包并解包验证；无预置consumer锁的Rust1.85消费者从解包产物运行guarded drift、数学数值和长Unicode往返成功，依赖来自registry。
- 独立只读审查无阻塞问题。没有重跑远端跨平台CI；未发布crate。

[checks.json](checks.json)保存最终命令/退出码/完整输出，以及明确分开的早期检查。各类语料可能重叠，不相加为独立覆盖率。源码hash与最终计时快照已核实一致。

## 仍需解决的差距

- guards候选仍逐个模拟完整文档，独立父节点很多时仍有二次成本。下一轮若做增量重放，必须先证明prefix与受影响suffix的依赖边界，以同一100/400对象、跨Copy、数组移位和guard字节等价验收；本轮不添加缓存层。
- 小对象完整流程仍慢于JS。诊断小配置 parse_pair_and_drop约7.426μs / pipeline10.332μs，约72%，是单独阶段测量，不把不相加的分量凑成精确总和。调用者已有Value可复用当前API；暂未有保持数值合同的替换parser实证，不关闭精度。
- 此轮没有重新比较所有功能、所有语言或真实下游负载。其它操作（fuzzy、merge、应用、复杂自定义matcher）的历史能力不能用此轮diff数字证明全面领先。

## 复现

```sh
python3 tools/remaining-hotspots.py /path/to/afe1868 /path/to/candidate /path/to/new-output
cargo test --locked
cargo test --release --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
cargo +1.85.0 check --lib --locked
cargo build --release --examples --locked
npm ci --prefix tools --ignore-scripts --no-audit --no-fund
node tools/interop.mjs
cargo +nightly fuzz run core -- -max_total_time=60 -max_len=2048 -rss_limit_mb=1024 -seed=20261001
cargo package --allow-dirty --locked
```

归档含最终源码快照、两个Rust probe、pinned npm/Go runner与锁、fixtures、所有阶段诊断、checks及MANIFEST。源码不含生产unsafe；分配探针为开发测量代码，仅用GlobalAlloc必需的unsafe透明转发System，计时程序不加载它。
