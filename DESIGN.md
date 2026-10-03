# Palim：JSON diff / patch 核心库设计

日期：2026-10-01。范围：可逆 JSON 变更、标准补丁、Merge Patch、可配置比较报告。
实施前跨生态调查保留在 [COMPARISON.md](COMPARISON.md)，性能见 [BENCHMARK.md](BENCHMARK.md)。

## 功能与验收

| 能力 | 行为 | 验收方式 |
|---|---|---|
| JSON 树 | 所有 JSON 类型与根变化；无变化返回 None | 固定例子、随机文档 |
| 数组身份与移动 | 稳定 ID、位置、精确值、自定义配对；移动后编辑 | 插入/删除/重排/重复 ID、JS 互通 |
| 唯一项最少移动 | LIS；纯排列的单项移动数为 n−LIS | 长度 2–7 全部 5,906 非恒等排列 |
| 过滤 | 对象属性、根、缺失节点、原位置数组投影 | 固定例子、独立随机投影 |
| 可逆 delta | jsondiffpatch 格式、strict patch/reverse/unpatch、serde | 正逆还原、reverse twice、外部 delta |
| 文本 | 多 hunk UTF-16 坐标；默认精确，显式模糊应用 | Unicode、偏移、失败原子性、坏坐标 |
| 前向精简 | 单独 ForwardDelta，可省旧值，不提供 reverse | 类型与正向还原 |
| 标准 diff | 顺序 move；跨路径 move/copy；字节优化；可选 test | 全部选项组合、独立应用器 |
| 标准应用/求逆 | 六种操作、数学数字 test、覆盖目标恢复 | 公共 RFC 用例、随机操作序列 |
| Merge Patch | RFC 7396 apply/diff/可表示的 compose | RFC 示例、独立 oracle、性质测试 |
| 比较报告 | 无序多重集、容差、自定义相等、身份、相似度、预算 | 穷举最大匹配 oracle、随机案例 |
| 数字 | 任意精度；数学相等；原生保留表示变化 | 超 u64/u128、巨大指数、哈希一致性 |
| 所有权/资源 | 消耗式应用、原子原地 API、copy 字节预算 | 结果一致、错误不修改输入、预算临界值 |

## API 与合同

包名 `palim`，库名 `palim`。发行记录见 [GitHub Releases](https://github.com/majiayu000/palim/releases)，API 文档见 [docs.rs](https://docs.rs/palim)。
完整 API 与字段说明见 [README](README.md)。选项归实例或调用所有，没有全局状态。

`Delta` 是完整可逆变更。`reverse` 只读原 delta，不重新 diff；生成时复制必要旧值。
`ForwardDelta` 显式省略旧值，不提供逆操作；wire 无法区分真实旧值 0 与省略占位符，必须使用对应类型解码。
JSON 不存在 absent root，native 根删除报错；两份真实文档的根变化生成 replace。

标准优化 `diff_json_patch` 输出 RFC 6902；实例方法沿用该实例的匹配和过滤投影。
`invert_json_patch` 读取原操作逐步执行前的必要旧值，支持覆盖目标与祖先移动，逆向未必是一条对应操作。
标准求逆需要基线，native 求逆不需要基线。

比较报告单独承担忽略顺序、容差和自定义相等语义，不承诺把原始 A patch 成原始 B。
无序数组保留重复次数；有序身份比较报告索引变化，不是最少移动脚本。
报告序列化区分缺失与 JSON null。Similarity 是匹配终端/缺失/类型变化单位的比例；空容器计一单位，每个 move 增加一个未匹配单位，过滤不计，空比较为 1。

消耗式应用复用未变化的原节点，避免初始整树 clone。原子原地 API 在私有结果成功后赋值；不是零复制原地修改。
copy 预算按每次复制前源值的序列化 UTF-8 字节累计；move 不计。预算耗尽不返回部分输出。
身份与过滤 callback 是调用者代码，需要确定且无副作用；panic 和分配失败不捕获。

## 算法与边界

对象递归比较。数组先 intern 匹配键：两侧唯一时 O(n log n) LIS 找稳定子序列；重复 token 裁剪共同前后缀后，候选相等对不超过 16,384 时用精确 LCS，否则用 Histogram，再按队列配对剩余移动。精确路径按 source 位置逆序枚举候选并求严格 LIS，空间由候选上限约束，没有 n×m 表。
身份优先；无身份的对象默认位置配对，可关闭；其它情况按完整 JSON 值。
自定义 matcher 对候选最大一对一配对，再算 LIS；歧义配对不保证全局最小成本，候选全比较可有二次成本。

native 数组把删除/移动放在 `_source`，新增/修改放在目标位置。应用先抽取移动，再一次重建数组并应用子修改。
逆补丁通过原/目标索引秩映射恢复位置。带旧值 move 检查基线，移动后有子编辑时 inverse 携带编辑后的旧值；空字符串 wire 占位符不能认证实际空字符串。
原生补丁操作索引用 BTreeMap，包含 O(n log(k+1)) 与值复制，不宣称严格线性。

标准数组由右向左安排移动，每个 native moved item 最多导出一次。原始槽位先占用，最终移动槽位先留空；最终槽位放在其下一个不移动项之前。Fenwick 前缀计数得到每步顺序索引，移动排序 O(n + moves × log n)，空间 O(n + moves)。无 move 不建计数树。公开 delta 导出仍应用一次验证基线，生成器已知目标时直接导出 typed operations。
factorization 按字节收益选择跨路径 move/copy，验证完整候选结果；rationalize 考虑父树到 root 的替换，使用实际 UTF-8 JSON 字节，包含可选 tests 成本。两者不是全局最优。
RFC 无 absence test，新增属性和数组插入保护父容器快照。
copy 搜索先按真实序列化成本排除无字节收益的候选，再按需建立标量到来源路径成本下界的缓存，数字用同源规范键。数组下标与数字对象键按一位数字计下界，避免删项使路径变短时漏候选；Move/Copy 的累计路径缩短量保守扣减所有成本，新 payload 继续按目标路径递归加入，历史值不删除。缺失或下界已超收益预算时不创建 shadow、不搜索；其它候选在当前文档查找。DFS 用 raw UTF-8 路径长度剪去不可能获益的分支，但保存和比较真实序列化成本，避免短 escaped 路径挡住更便宜的普通路径。顺序 shadow 仅在需要时创建和推进，保留后来新增值的 copy 能力。
rationalize 携带 tests 时，已验证独立子树可复用精确 GuardCost；外部祖先认证读、跨Copy/Move、数组移位等未证明情况仍完整模拟。接受父替换后的现有扫描仅在guarded模式合并独立性证明，根候选仍实际验证。仍有逐父标量扫描、前缀重建与操作压缩的二次成本；历史4.2秒探针及当前改进见 BENCHMARK.md 和候选扫描报告。

过滤在对应节点调用：数组是原位置投影，排除的位置保留左值或缺失；新增容器递归过滤新子节点，删除容器保留被排除的旧后代，替换为容器递归过滤其新后代。容器改标量由父节点授权，后代不再回调。嵌套数组过滤见原始目标下标，wire 与错误位置用投影后下标；缺失不会补成 null。
无序报告用最大二分匹配，候选工作计入预算，避免非传递容差的贪心误配。精确默认比较用私有结构指纹分桶；碰撞和粗指纹仍检查真实比较。

文本阈值达到所需 UTF-16 单元即停止计数；先用安全的 256 字节 slice 比较裁剪共同 UTF-8 前后缀，再修正字符边界。UTF-16 长度按每 8 字节分类 UTF-8 字节并 popcount，不含 unsafe 或平台分支。仅将变化中段和上下文转为字符序列。较大窗口先匹配行组，再细化变化区间；每次 Imara 调用的两侧 token 总数不超过 4096。字符细化先裁剪相同前后缀，超过上限时生成精确 replacement。行组大小按两边总行数向上取整，末尾不完整组也计入上限。插入一行可能改变后续组边界，产生更粗的补丁；不保证最小文本 delta。
wire 坐标按 UTF-16。默认精确应用；fuzzy 对短 anchor 做有界 Levenshtein，长 hunk 用首尾 anchor 与启发式对齐。
偏移按 UTF-16、误差按 Unicode scalar 计；长 hunk 可以误拒绝，不宣称复制 DMP 行为。失败丢弃私有结果，非文本旧值检查不放宽。

任意精度数字使用 serde_json 数字字符串与规范键：符号、有效十进制位、BigInt 指数。仅指数做大整数运算，不展开 1e1000000000。
native 保留数字表示差异；标准 test/report 比较数学值。非零 tolerance 按精确十进制判断 `|a−b| ≤ max(abs_tol, rel_tol × max(|a|,|b|))`。f64 阈值定义为最短 JSON 十进制值。常规数字先走 checked i128 缩放/减法/绝对值；不能表示或溢出时回到精确稀疏算法。三个带符号系数按稀疏十进制位相加：巨大指数间隙直接跳过，进位/借位稳定为 0/−1；相对阈值最多乘 17 位系数，不展开指数差。数值文本和 digit 工作计入原有 max_comparisons；BigInt 指数自身的运算成本还受指数文本长度影响。
Rust f64 转 JSON 已经选取最短十进制表示，需精确十进制时从 JSON text 解析。

Merge Patch 的 object member null 是删除，生成器对无法表达的新 null 成员报错。
compose 必须对任意基线成立；reset 后 object patch 等无法通用表示的组合明确报错，不用有限样本猜测。

文档默认最多 128 层容器，delta 含 tuple 包装最多 127 层，以通过 serde_json 默认 reader 往返。近边界源文档可能因 delta 更深报错。
检查 imara 的 i32 长度边界。标准初始文档检查一次深度；Add/Replace 检查插入子树，Move/Copy 仅向更深层插入时检查源子树。每步保留中间深度限制；组合深度错误在标准路径验证后报告，独立 payload 深度与 copy 字节预算保持原有优先次序。Remove/Test 不重复扫描整个文档。

## Adopt / adapt / build 决策

- **Adapt** [imara-diff 0.2.0](https://github.com/pascalkuthe/imara-diff)（Apache-2.0），用于重复项和文本，不复制源码。
- **Adopt** [json-patch 4.2.0](https://github.com/idubrov/json-patch) 的 Patch 类型与标准变更执行。**Adapt** 数学 test、根自身 move 和错误序号边界。
- **Adopt** serde_json `arbitrary_precision` 与 [num-bigint 0.4.8 官方 manifest](https://github.com/rust-num/num-bigint/blob/num-bigint-0.4.8/Cargo.toml)（MIT OR Apache-2.0，MSRV 1.60）表示指数；拒绝会展开巨大指数的计算方案。
- **Build** JSON 遍历、LIS/身份配对、delta 校验、一次数组重建、逆向映射、Unicode 协议、标准优化/求逆和比较报告。
- 协议参考 [jsondiffpatch 0.7.6](https://github.com/benjamine/jsondiffpatch/blob/master/docs/deltas.md)。生成标准操作直接构造 typed operations，绕过上游 tagged serde `from_value` 的 u128 缓冲限制；真实标准 wire 使用 `from_str`。

拒绝直接采用 Rust jsondiffpatch 0.1.0：已复现全局构造器状态、reverse 未实现与正向失败。
拒绝 fionn 的已测 LCS 路径：最小错误还原例子。spatch 身份模式不保证目标数组顺序。
仅用 json-patch diff 会为移位数组生成大量位置 replace，无法同时提供可逆 native、身份移动和文本 delta。
比较策略借鉴 DeepDiff，标准优化借鉴 wI2L/jsondiff，copy 预算借鉴 evanphx/json-patch；采取当前 JSON 要求需要的能力，不引入插件平台。

### 本轮优化选择

| 方案 | 决策与依据 | 风险与验证 |
|---|---|---|
| 标准导出静态槽位 + Fenwick | Build；最终顺序已知，采用 [Fenwick 原论文](https://doi.org/10.1002/spe.4380240306) 的前缀计数，拒绝通用平衡树和重复 Vec 移动 | 外部合法 delta、全项移动、混合删增改、完整小排列和 20k 重排 |
| 有界重复项精确 LCS | Adapt [Hunt/McIlroy 报告](https://mcilroy.cs.dartmouth.edu/diff.pdf) 的候选方法；保留大重复集合的 Histogram fallback，拒绝无界候选与 n×m 表 | 独立 LCS oracle、重复项纯排列与 fallback 往返；不保证任意 matcher 编辑成本 |
| 文本扫描与有界行组 | Build 安全 slice/popcount 优化稀疏单处修改；三处编辑压力测试发现 Imara 0.2 的重复 token 预处理退化，[官方修复](https://github.com/pascalkuthe/imara-diff/commit/f02e46a737c43c0d9c65a42e5e08046bd10fd739) 已合入但本次未采用未发布 Git 依赖。采用行组匹配与小区间字符细化，拒绝 vendor 整套引擎与无界字符 diff | Unicode、CRLF、分组边界、长单行、100 个独立长文本 JS 正逆互通；大区间精确但更粗。该 token 上限不覆盖 fuzzy alignment 或数组 Histogram fallback |
| RFC 成本与按需状态 | Adapt [wI2L 官方优化器](https://github.com/wI2L/jsondiff/blob/master/differ.go) 的收益先算思路；counting writer 保留真实 escaping 成本，收益足够时才构造候选 | 优化选项组合、later-added copy、预算/深度合同、统一 pipeline 与分配计量 |
| 精确十进制 tolerance | Build 稀疏带符号位运算，复用现有 NumberKey/BigInt；拒绝展开巨大指数的 rescale 和新阈值配置 | 十进制边界、抵消/异号、巨大正负指数、预算及独立整数 reference |
| 借用 delta / RawValue / 全局数值缓存 | 延后；已有 owned API 避免整树初始 clone，优先复用已知目标。缓存对重复小数有收益，对唯一小数及整数反增分配 | 分配数据单独记录；没有真实下游和统一收益证据时不新增 API、parser 或缓存层 |
| 结构化 fuzz / 三 OS CI | 最小新增；参考 [Rust Fuzz 官方指南](https://rust-fuzz.github.io/book/cargo-fuzz/guide.html)，生成可执行协议结构，CI 复用本库实际检查 | 本地实测与未来远端运行分开；跨 target compile 不等于 OS runtime 验证 |

## 验证与限制

主要风险：混合数组移动与逆向、重复身份、外部恶意 delta、模糊文本偏移、标准覆盖目标、大数字。
用确定性回归、真实协议结构生成器、固定种子性质测试、独立应用器、JS 双向互通、统一边界 benchmark 验证。
旧只读评估和旧原型不能代替本轮源码验证，也不能将部分简化型输出的时间当完整新库性能。

HTML/CSS、浏览器动画、产品 CLI、Python/Node/WASM 绑定、OT/CRDT、任意语言对象、扩展查询语言在当前边界外。
重复项最小 delta、全局最小字节、与 DMP 完全相同的 fuzzy 策略和所有生态最快没有证明，不作承诺。

## 实际互通确认

外部 JS text hunk 上下文可重叠，逆向从最后 hunk 开始撤销。
`@dmsnell/diff-match-patch` 1.1.0 的 surrogate 修复可能使头部两个长度同时偏一单位；解码按真实操作归一，仍检查净长度差与归一后的坐标范围，杜绝 reverse 溢出。
URI 解码保留 JavaScript decodeURI 的 reserved escapes。默认严格路径不会用 fuzzy 隐藏错误协议。
标准失败保留实际操作序号和路径。上游原地非原子入口仅用于不可见 scratch，任何错误都丢弃它。
