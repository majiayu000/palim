# Palim：同类库功能、正确性与性能对比

本库已更名为 **Palim**（包名 `palim`）。以下历史测量与验证记录保留当时的名称 `jsondiffpatch-rs`，未因更名重新计时。

## 2026-10-02 文本匹配修复

默认 fuzzy 已修复两个合法 JS 逆补丁的重复文本匹配失败，原始补丁未修改。
JS 自逆成功的 566 个案例中，562 个格式合法案例现在全部还原；4 个畸形补丁
仍明确报错。必需的 1,073 组互通路径全部通过。
详见 [验证记录](results/text-inverse-20261002.json)。改动尚未发布，
冻结语料通过不构成任意 DMP 模糊行为完全兼容的证据。

## 2026-10-01 发布后核查

**0.1.0 已发布，三 OS CI、MSRV、JS 互通与 fuzz 已在远端实际通过。**
[发布记录](https://github.com/majiayu000/palim/releases/tag/v0.1.0)、
[CI 结果](https://github.com/majiayu000/palim/actions/runs/36841181994)。
下方关于未发布或 CI 尚未运行的文字属于发布前快照。

本次工作区修复了重复 ID、补丁优化和 guards 的已复现慢路径：同一
预解析输入的定向中位数从 4257.794 ms 降至 10.736 ms，补丁仍为 88,590 B。
16 个固定标准输入的对照未出现补丁体积增加，正式带 guards pipeline 的
48 次测量和独立应用验收全部通过。详见 [BENCHMARK.md](BENCHMARK.md)。
这些结果不构成跨语言、跨平台或任意输入的性能第一证据。

文本兼容已分别统计 Rust 生成的逆补丁、JS 自己的逆补丁、严格应用与显式 fuzzy。
该轮在 JS 自逆成功的 566 个案例中，默认 fuzzy 还原 560 个，4 个逆补丁结构不合法，
2 个合法重复文本不能还原原文；这两例已于 10 月 2 日修复。可靠 undo 使用导入的正向 delta 加 Rust
`reverse`/`unpatch`；不把近似匹配或畸形输入写成完全兼容。

当前已发布版本有完整的公开安装路径和持续验证；真实业务采用尚未由本次核实。
库内功能组合与特定负载已有优势，仍不能宣称全面优于全部竞品。

## 2026-10-01 实现更新

> 以下是发布前的实现更新与验证快照；发行状态以本页上方的发布后核查为准。

本节对应新增实现后的源码。下方 2026-09-30 报告保留为**实施前基线**；其中“当前缺少”“当前有缺陷”和旧性能数字均指当时版本，不能继续代表本轮源码。

本轮将已确认的 JSON 核心缺口落实为 API、测试与基准。下表明确记录功能覆盖和语义边界；实际执行记录见 [verification.json](results/verification.json)，新性能结果见 [BENCHMARK.md](BENCHMARK.md)。

### 功能覆盖更新

| 能力 | 本轮已实现 | 验收证据与准确边界 |
|---|---|---|
| 有序数组身份与自定义配对 | 原有 `object_hash` 之外，native diff 新增 `array_item_matcher`，可以配对改变后的标量或容器；歧义候选采用确定性的最大一对一匹配，再选择稳定子序列 | [数组用例](tests/array_quality.rs)：改变的标量、歧义匹配、混合增删改移动。native 仍保留目标顺序；matcher 与 object_hash 在 native 入口互斥。任意 matcher 需要检查候选对，不具有大型数组线性成本保证 |
| 无序与自定义比较 | 新增独立 `compare` 报告 API，支持按路径选择无序数组、多重集重复计数、身份配对、自定义相等、相似度以及比较/差异预算 | [比较用例](tests/compare.rs)：重复值、嵌套无序容器、非贪心最大配对、过滤和预算。`CompareReport` 不是 patch；无序和容差不会进入 native 可逆 delta。报告的 moves 是身份项索引变化，不是最短移动脚本 |
| 根、缺失节点与数组过滤 | `node_filter` 覆盖根、对象增删、数组位置；原有 `property_filter` 保留。native 数组先按原位置投影，排除的位置保留 source 值，再进行身份配对 | [投影与调用次数用例](tests/array_quality.rs)、[报告过滤用例](tests/compare.rs)。native 过滤后的 patch 目标是投影结果，不必等于未过滤的 B；数组项根只进行一次投影过滤，嵌套字段继续过滤并见原始目标下标。新增容器和容器目标类型变化递归过滤后代；整体删除可保留排除的后代；容器改标量由父节点授权。报告配对后的路径语义与 native 投影不同 |
| 精确数值与容差 | 启用 serde 任意精度数值；标准 `test` 和比较报告按十进制数值递归比较，支持巨大整数、小数和符号指数；报告可选绝对/相对容差 | [数值用例](tests/numbers.rs)、[标准语义用例](tests/standard_semantics.rs)。精确比较不先转 f64。容差按精确十进制不等式计算，f64 阈值表示其最短 JSON 十进制值；巨大正负指数保持符号形式，数字工作计入比较预算。native diff/旧值校验仍保留 serde 表示语义，不把 `1` 与 `1.0` 的表示变化吞掉 |
| 有序数组移动质量 | 唯一 token/身份使用 LIS；RFC 导出按目标顺序从右侧安排 native moves，避免重复移动同一项 | [移动数量用例](tests/array_quality.rs) 枚举长度 2–7 的 5,906 个非恒等唯一排列，并要求 native/RFC 都达到 `n−LIS`；500 项半区交换要求 250 次。该最少只针对固定唯一项、单元素移动、无整树 replace 的成本模型；重复 token 在内部候选预算内用精确 LCS，超预算仍用 Histogram，任意自定义配对和一般 JSON 不保证全局最少操作/字节 |
| RFC 6902 补丁缩小 | 新增 `diff_json_patch`，支持跨路径 remove/add→move、复用当前文档值的 copy，以及按实际序列化 UTF-8 字节比较父节点 replace | [RFC 扩展用例](tests/rfc_extended.rs)：跨属性/数组、重复值、交错索引、嵌套 rationalize、转义与 UTF-8。rationalize 计入请求的 guards；候选通过重新应用验证。优化是顺序启发式，不是全局最小字节算法；copy 转换并不覆盖所有数组 replace 情形 |
| RFC guards 与保存逆补丁 | 可在标准补丁中生成旧值/来源/容器 `test`；`invert_json_patch` 根据原操作与完整基线生成可保存的 RFC 逆补丁，处理覆盖、append、跨数组 move 和祖先目标 | [RFC 扩展用例](tests/rfc_extended.rs)。标准逆补丁需要基线；native reverse 仍无需基线。guard 对缺失键、数组插入测试父容器，可能较大且拒绝同容器中其它变化；`test` 本身不产生逆操作 |
| RFC 7396 | 新增 Merge Patch 应用、原子原地应用、生成与可表示的补丁组合 | [RFC 附录与属性用例](tests/merge.rs)。对象成员 null 表示删除，因此新增/改成 null 的成员不能由生成器表示；根 null 和数组内 null 可表示。对未知基线无法保持语义的组合返回 Error，不声称任意两个 merge patches 都可合并 |
| 有界文本生成与可选模糊应用 | 多 hunk UTF-16 协议；大文本先匹配行组，再细化小变化区间，每次 token diff 两侧总数至多 4096；显式 `apply_text_patch` / `patch_fuzzy` 支持位移与误差界限 | [文本扩展用例](tests/text_extended.rs)、[独立长文本互通](results/review/text/review-summary.json)。大区间可生成更粗但精确可逆的 replacement，不保证最小文本 delta。生成的 token 上限不覆盖 fuzzy；默认 native patch 仍精确，fuzzy 的非文本操作仍严格。坐标/位移使用 UTF-16、误差按 Unicode scalar；不承诺 DMP matching、任意 JS inverse 或模糊行为完全兼容 |
| 所有权与资源约束 | native/RFC 新增消费 baseline 的 owned 应用与成功后提交的原子 in-place API；标准应用可限制 copy 的累计序列化字节和深度 | [应用用例](tests/application.rs)。in-place 在私有结果上完成后提交，并不等于零拷贝原地编辑；没有新增借用 delta、流式输入或 Tape API |
| 已发现的错误路径 | 标准数值 `test`、根 self-move、文本归一化后的坐标范围已修正；带非占位旧值（不为 `""`）的 move 参与严格校验，逆向保留配套值 | [标准语义](tests/standard_semantics.rs)、[文本溢出回归](tests/text_extended.rs)、[协议感知生成/变异](tests/protocol_properties.rs)。空字符串仍是 move placeholder，不能认证整个基线；其它未变化字段也不会自动接受完整基线认证 |

### 可以验证的特色与仍弱的部分

- **特色是能力组合。** 一个 Rust JSON 核心现在同时提供可逆 native delta、稳定身份/自定义有序配对、独立无序/容差报告、RFC 6902 优化与逆向、RFC 7396，以及可选有界模糊文本。新增用例提供具体可复查的验收，不构成“功能全超越所有竞品”的证据。
- **移动最优性有明确范围。** 唯一 token 路径针对纯重排的单元素移动达到 LIS 成本；重复项在有界精确 LCS 路径也获得最长稳定子序列；超预算的启发式路径、歧义身份的编辑成本、自定义匹配和任意 JSON 最小字节仍没有全局保证。RFC 导出使用 Fenwick 计数，移动排序 O(n + moves·log n)，不等于完整应用与优化的总成本。
- **比较策略已补齐常见 JSON 缺口。** 与 [DeepDiff](https://zepworks.com/deepdiff/current/diff.html)、[dictdiffer](https://github.com/inveniosoftware/dictdiffer)、[SystemTextJson.JsonDiffPatch](https://github.com/weichch/system-text-json-jsondiffpatch) 重叠的过滤、自定义比较、容差和无序能力已有 API；不代表已复制其所有报告、策略、缓存或格式行为。数值和容差运算使用精确十进制，阈值 API 的 f64 被定义为最短 JSON 十进制值。
- **标准补丁质量已增加相应能力。** 与 [wI2L/jsondiff](https://github.com/wI2L/jsondiff)、[zjsonpatch](https://github.com/flipkart-incubator/zjsonpatch) 重叠的 factorize、rationalize、guards/inverse 已有实现；优化顺序、成本、可接受候选与接口仍不相同，尚不能宣布标准输出时间或补丁体积整体优于它们。
- **DMP 行为兼容仍有限。** [JS jsondiffpatch](https://github.com/benjamine/jsondiffpatch) 和 .NET 对应实现的 DMP matching 与本库有界近似匹配不同。历史严格文本逆向拒绝的 95 个案例不能直接写成全部已修复；需要对新版分别核查 exact 与 fuzzy，保存实际完成的互通结果。
- **输入/系统边界仍不同。** 本库没有 fionn 的 Tape/借用输出、真正流式输入，以及 JS plugins/formatters、观察器、二进制格式、codegen 或语言绑定。Python 任意对象、OT/CRDT 和协作冲突引擎也超出当前 JSON 核心。它们不能作为本库必须增加的功能清单，也不能被写成已覆盖。

### 产品定位与结论

**面向 Rust 的可逆 JSON 变更与比较库，重点处理稳定身份的有序数组，并提供 RFC 6902/7396 接口和可选比较策略。** native 的精确变更、宽松的比较报告、基线依赖的标准逆向，以及可选 fuzzy 各自具有清楚合同。

此前查到的 kube/Tauri、DevTools、历史撤销与差量同步使用方仍能说明不同需求存在，不能证明这些用户已经采用本库。ID 在生成阶段用于配对，原生 delta 在应用阶段仍按索引寻址；本库仍不是不同基线上的自动合并引擎。

**新版性能已单独测量。** 94 项 Criterion、36 个原生横向测量（另 4 项预算跳过），以及标准 pipeline/export/apply/inverse 共 216 个独立进程测量均完成，见 [BENCHMARK.md](BENCHMARK.md)。下方数字仍是实施前记录。大型移动、单处文本 diff 与补丁体积有实测优势，多重复身份、全异数组、小对象及部分应用/生成边界仍有更快的竞品反例。最终 ASan fuzz 与四个 target 编译通过，三 OS CI 尚未远端运行；未发布、没有已核查真实下游，尚不能宣称全面超越。

## 历史分析（2026-09-30，实施前基线）

> 以下全文描述 2026-09-30 调查时的实现，保留原始缺陷、对比和测量证据。它不描述 2026-10-01 新增能力后的当前状态；“本轮”“当前”均应按历史日期理解。

日期：2026-09-30。目标版本：本地 `jsondiffpatch-rs 0.1.0`。

### 结论

**现在不能称它为同类最好的库，也不能称功能最全。**

它已经实现了一组有区别的能力：可逆 JSON delta、对象身份匹配、有序数组移动、移动后的字段编辑、Unicode 文本增量，以及 jsondiffpatch 格式互通。已有实测支持它在特定大型数组移动上明显优于 JS 原版。

但本轮还确认了两个正确性缺陷、标准根路径边界、RFC 导出放大问题，以及多项功能缺口。其他 Rust 库的生产采用更成熟；跨语言竞争实现尚未做统一实测，不能给全生态性能排名。

**更准确的定位是：面向 Rust 的可逆 JSON 变更库，重点处理稳定身份的大型有序数组。** 在本次查到的已发布 Rust 库中，这组能力较完整，这是有证据约束的判断，不是“全部 Rust 项目第一”的事实。

### 范围与方法

- 启动 13 个独立研究 agent，分别调查 Rust、JS、Python、Go/Java/.NET、C/C++、数组、文本、标准、算法、benchmark、测试成熟度、逆向互通和真实使用方。
- 使用官方 registry、官方发布源码、技术文档和 RFC；配置字段、README 宣称不直接算已实现能力。
- 同时核查原生 JSON delta、RFC 6902、RFC 7396、比较报告和协作系统的边界。RFC 7396 已取代 RFC 7386。
- 新增有限正确性验证和一次串行 Rust 对照测量。生产 Rust 源码未改，六个文件 SHA-256 与此前交付记录一致。
- 本轮的复现源码、结果、原始时间样本和源码哈希保存在 [comparison-review.json](results/comparison-review.json)。没有因只读对比重跑整套 Cargo gates；此前完成的检查见 [verification.json](results/baseline-20260930/verification.json)。

“最好”必须具体到正确性、速度、补丁体积、语义、使用成本和成熟度；这些指标可能互相冲突。不能把最高单项速度倍率或最多功能数当作综合结论。

### 1. 已确认的正确性问题

#### 1.1 RFC `test` 错误区分 1 与 1.0

**已运行验证。**

```json
doc = {"x": 1}
patch = [{"op":"test","path":"/x","value":1.0}]
```

本库返回 `value did not match`。嵌套对象、数组里的 `1` 与 `1.0` 也失败；同为整数的控制例通过。

[RFC 6902 §4.6](https://www.rfc-editor.org/rfc/rfc6902#section-4.6) 要求数字按数值相等比较，对象和数组递归适用。问题来自 [src/lib.rs](src/lib.rs#L203) 复用的 json-patch 4.2.0：依赖 `test()` 用 serde_json Value 相等，而该类型区分整数与浮点存储变体。

因此，支持六类操作入口、92 个公共协议用例通过，都不等于完整 RFC 合规。原生 diff 的 serde 数值语义已有文档说明；这里需要修正的是标准 `test` 语义。

#### 1.2 畸形外部文本 delta 可令 reverse panic

**已运行 release 和独立 debug probe。** 64 位最小输入：

```json
[
  "@@ -18446744073709551615,0 +0,0 @@\n-a\n+b\n",
  0,
  2
]
```

`Delta::from_value` 接受此输入，`patch("a", delta)` 得到 `"b"`。随后：

- debug 构建的 `reverse()` 在 `src/text.rs:36` 因 `start + 1` 溢出而 panic；probe 捕获到了 panic。
- release 构建的坐标回绕，逆向头部不合法，应用逆补丁失败。

原因是 [text.rs:142](src/text.rs#L142) 先容许声明长度与真实操作长度的等量偏差，再用真实长度归一化，却仍用原声明长度检查坐标范围。`reverse()` 直接包装结果，不会重新拒绝这个非法头部。

这属于错误输入处理缺陷，不是正常生成文本路径的失败。应在归一化后检查范围，保证坏输入返回 Error、成功 reverse 返回可用 Delta。

#### 1.3 根路径 move 到自身被拒绝

**已运行验证；标准判断依据 RFC。**

```json
[{"op":"move","from":"","path":""}]
```

本库返回 `from path is invalid`。依赖先 remove 根路径，而其 remove 不支持空 pointer。[RFC 6902 §4.4](https://www.rfc-editor.org/rfc/rfc6902#section-4.4) 只禁止移入自身子节点，根移到自身没有违反 proper-prefix 条件。应补齐该无变化操作，或明确标准入口的限制。

#### 1.4 带旧值的 move 也不检查旧值

**已运行验证；行为限制，不直接等同结果错误。**

```json
left = [1,2]
delta = {"_t":"a","_0":[999,1,3]}
```

应用成功得到 `[2,1]`。移动取当前 source 项，不检查 delta 中的 999。`include_value_on_move` 只是保留数据，不增强基线认证。[patch.rs:90](src/patch.rs#L90)

README 目前只明确 placeholder move 不认证基线，说明应扩展到所有 move。身份回调用于 diff 匹配，patch 仍按索引寻址；它不是不同基线上的安全合并机制。

### 2. Rust 生态

| 项目 | 核查到的能力与边界 | 与本库的区别 |
|---|---|---|
| [json-patch 4.2.0](https://docs.rs/json-patch/4.2.0/json_patch/) | RFC diff/apply、Merge Patch、原地应用和回滚；数组 diff 按位置 | 标准协议和生产采用更成熟；没有本库的身份数组、文本增量和原生 reverse |
| [jsondiffpatch 0.1.0](https://github.com/soraxas/jsondiffpatch) | JS 格式移植，reverse 仍返回 None，构造器全局状态有限制 | 不是一个已完成的全功能替代方案 |
| [fionn-diff 0.2.0](https://docs.rs/fionn-diff/0.2.0/fionn_diff/) | diff/apply、Merge Patch、Tape、借用输出、其它格式接口 | 功能面更广，但不是可逆 jsondiffpatch delta 引擎 |
| [spatch 0.6.0](https://github.com/kamilczerw/spatch) | schema 身份、扩展路径、补丁粒度、CLI | keyed 数组按身份集合处理，不重建目标顺序 |
| [jadipa 0.3.1](https://github.com/buehler/jadipa) | Pointer、RFC diff/apply、Merge Patch 应用 | 未查到身份、文本 delta 或原生 undo API |
| [jsondiff_rs / jsonpatch_rs](https://github.com/Nero5023/jsondiffpatch.rs) | LCS 结构 diff 与标准应用，另有 CLI | 没有本库的原生可逆格式、身份 callback 和文本 delta |
| [json-patch-ext 0.3.3](https://docs.rs/json-patch-ext/0.3.3/json_patch_ext/) | 数组通配路径、自动建父路径 | 标准补丁器扩展，不是同边界的差异引擎 |
| [json-structural-diff 0.2.0](https://docs.rs/json-structural-diff/0.2.0/json_structural_diff/) | 相似度、keys-only、数组匹配、彩色输出 | 没有公开 patch/reverse；不是完整变更引擎 |
| [diff_json 0.1.1](https://docs.rs/diff_json/0.1.1/diff_json/) | 按位置或忽略顺序比较、多种 formatter | 没有公开 patch/reverse |
| [serde_json_diff](https://docs.rs/serde_json_diff/0.2.0/serde_json_diff/)、[json_diff_ng](https://docs.rs/json_diff_ng/0.6.0/json_diff_ng/)、[sjdiff](https://docs.rs/sjdiff/0.0.7/sjdiff/)、[json_diff_rs](https://docs.rs/json_diff_rs/0.0.5/json_diff_rs/) | 结构比较、部分过滤或排序、诊断输出 | 主要输出比较结果，不是 diff→patch→undo 核心 |

几处源码核查改变了 README 印象：

- fionn 的 `detect_moves/detect_copies` 有配置字段，但发布版生成路径没有读取；不能计为自动识别已实现。[compute.rs](https://docs.rs/crate/fionn-diff/0.2.0/source/src/compute.rs)
- fionn 的 `three_way_patch(base,target)` 只有两份文档，没有第三分支和冲突处理；不能计为三方合并。[tape_patch.rs](https://docs.rs/crate/fionn-diff/0.2.0/source/src/tape_patch.rs)
- spatch 的 `/users/[id=u-2]/name` 是扩展路径，不是通用 RFC 6901 应用器可直接解释的路径。
- [similar](https://docs.rs/similar/3.2.0/similar/)、[assert-json-diff](https://docs.rs/assert-json-diff/)、[serde-diff](https://docs.rs/serde-diff/)、[struct-patch](https://docs.rs/struct-patch/) 是邻接组件，边界不同，不应混成一个排名。

官方反向依赖记录中 json-patch 已有 kube/tauri 相关使用。当前本库未发布，也没有核查到真实下游采用。下载量和反向依赖数不能直接证明算法质量。[官方依赖 API](https://crates.io/api/v1/crates/json-patch/reverse_dependencies?page=1&per_page=5)

### 3. 跨语言竞品

| 项目 | 与本库直接相关的能力 | 需要保留的比较边界 |
|---|---|---|
| JS [jsondiffpatch 0.7.6](https://github.com/benjamine/jsondiffpatch) | 身份、move、可逆 delta、DMP、模糊文本应用、plugins、formatters | 本库精确文本语义更窄；UI/CLI 不是底层算法质量指标 |
| JS [fast-json-patch 3.1.1](https://github.com/Starcounter-Jack/JSON-Patch) | 标准应用、compare、观察变化；compare 可加旧值 test | compare 不生成 move；旧值 test 不等于内置 undo API |
| JS [json-joy](https://github.com/streamich/json-joy) | JSON Patch+、谓词/字符串等扩展、紧凑/MessagePack 编码、codegen | OT/CRDT 是独立系统；主包当前 AGPL-3.0-only，子包许可证不同 |
| Python [DeepDiff 9.1](https://zepworks.com/deepdiff/current/diff.html) | 按路径忽略顺序、重复计数、数值容差、自定义比较、DeepHash/cache、双向 Delta | 比较策略明显更广；Python 自定义对象不属于本库 JSON 范围 |
| Python [jsondiff](https://github.com/xlwings/jsondiff) | 多种 delta、symmetric undo、相似度 | 数组匹配分配二维矩阵，有规模成本；未同机测速 |
| Python [dictdiffer](https://github.com/inveniosoftware/dictdiffer) | patch/revert/swap、相对与绝对容差 | 列表以位置处理，不等于稳定 ID move |
| Python [jsonpatch](https://github.com/stefankoegl/python-json-patch) | RFC diff/apply，部分 move 归并 | 标准单向 patch，不是同协议可逆 delta |
| Python [json-delta](https://json-delta.readthedocs.io/en/latest/python.html) | 按编码成本选择子树替换，另有 udiff | 普通 delta 和可逆 udiff 是不同格式 |
| Go [wI2L/jsondiff](https://github.com/wI2L/jsondiff) | move/copy 因式分解、按字节 rationalize、无序等价、test/invert、Merge Patch 生成 | 没有公开 apply API；部分 LCS 仍用二维矩阵 |
| Go [evanphx/json-patch](https://github.com/evanphx/json-patch) | RFC 应用、Merge Patch 生成/应用/合并、copy 增量预算 | 没有 RFC 6902 diff 生成器 |
| Java [zjsonpatch](https://github.com/flipkart-incubator/zjsonpatch) | 跨路径 move/copy、test 生成、旧值扩展、原地/复制应用 | 键选择指针是扩展；LCS 与归并仍有二次成本 |
| Java [java-json-tools/json-patch](https://github.com/java-json-tools/json-patch) | RFC diff/apply、move/copy、Merge Patch、数学数值 test | 最近源码活动较旧；没有核实独立可逆 delta API |
| .NET [JsonDiffPatch.Net](https://github.com/wbish/jsondiffpatch.net) | 原生 delta、undo、ObjectHash/move、路径忽略、DMP fuzzy、RFC 导出 | 当前源码已有 move，不能沿用旧 benchmark 的相反注脚 |
| .NET [SystemTextJson.JsonDiffPatch](https://github.com/weichch/system-text-json-jsondiffpatch) | 原生可逆 delta、身份、任意值比较器、文本 provider、过滤、RFC 输出 | 自定义比较比本库广；旧机器上的性能数字不可拼入本机排名 |
| .NET [Ivy.NativeJsonDiff](https://github.com/Ivy-Interactive/Ivy.NativeJsonDiff) | Rust FFI diff/apply | 内部直接用 json-patch，不是另一套自研 Rust 引擎 |
| C++ [nlohmann/json 3.12.0](https://json.nlohmann.me/api/basic_json/diff/) | RFC diff/apply、Merge Patch 应用、原子复制与原地 API | diff 按位置，只生成 add/remove/replace |
| C++ [jsoncons 1.9.0](https://github.com/danielaparker/jsoncons/tree/v1.9.0/include/jsoncons_ext) | RFC diff/apply、内部错误回滚、Merge Patch 生成/应用 | 内部 undo 栈不是公开可保存 reverse API |
| C [cJSON Utils 1.7.19](https://github.com/DaveGamble/cJSON/blob/v1.7.19/cJSON_Utils.h) | RFC diff/apply、Merge Patch 生成/应用 | apply 非原子；diff 会排序输入；标准比较须用大小写敏感 API |
| C [yyjson 0.13.0](https://ibireme.github.io/yyjson/doc/doxygen/html/api.html) | 复制后 RFC apply、Merge Patch apply、结构化错误 | 无公开 diff/身份/undo/text 生成接口 |

另查了 TS [rfc6902](https://github.com/chbrown/rfc6902)、C++ [jsondiff-cpp](https://github.com/BlockLink/jsondiff-cpp)，以及 [APTED](https://github.com/DatabaseGroup/apted)、[JEDI/QuickJEDI](https://github.com/DatabaseGroup/tree-similarity)。它们分别是不同补丁成本模型、维护证据较弱的实现和树距离研究，不构成本库的直接全功能替代。

JS 原版确有标准应用入口，但位于 `jsondiffpatch/formatters/jsonpatch` 的 `patch`；根入口 `patch` 消费 native delta。源码 switch 有六类操作，本轮完成了简单公开入口验证，但没有因此宣称完整合规。[官方导出](https://github.com/benjamine/jsondiffpatch/blob/a60db8a232f92ee4b987c14f43f77402885d1a5a/packages/jsondiffpatch/src/formatters/jsonpatch.ts#L250)

### 4. 当前缺少的功能，哪些值得补

| 能力 | 官方已有参照 | 与当前可逆 JSON 目标的关系 |
|---|---|---|
| 更少、更小的标准补丁 | wI2L Factorize/Rationalize、zjsonpatch | 直接相关；先改善当前 exporter，再考虑跨路径 move/copy |
| RFC 旧值 test 输出 | fast-json-patch、wI2L、zjsonpatch | 第三方应用补丁时有价值；当前导出失去 native 的旧值校验 |
| Merge Patch | json-patch、Go/C/C++ 实现 | 独立需求，不能仅为“最全”重复依赖已有能力 |
| 任意节点路径过滤 | DeepDiff 等 | 当前只在对象属性调用过滤；数组元素、根节点覆盖有限 |
| 自定义比较、容差 | DeepDiff、dictdiffer、SystemTextJSON | 会改变精确恢复目标的合同，应先确认需求 |
| 无序数组/多重集比较 | DeepDiff、diff_json | `[1,2]` 与 `[2,1]` 可报告无变化，但无法再承诺 patch 精确得到原始 B |
| 模糊文本应用 | JS/.NET DMP | 不同基线用途；当前严格语义是合理选择，但不是全兼容 |
| 原地/借用/流式接口 | 部分标准库、fionn | 所有权和错误原子性要另行衡量，尚无真实需求证明必须增加 |

插件、HTML、CLI、二进制编码、OT/CRDT、语言绑定均不是当前 JSON 核心完成的必要条件。功能数量不能代替正确性；本轮不以“最全”为理由把这些加入实现。

### 5. 补丁质量与算法

#### 5.1 Native Histogram 不保证最少移动

对长度 2–7、唯一元素的全部非恒等排列，共 5,906 对，Rust 与 JS 正向、逆向全部正确。但 Rust 有 1,573 对 native moves 超过理论最少，JS 0.7.6 为 0。

```text
[0,1,2,3] → [3,0,2,1]
Rust：3 moves，52 B
JS：  2 moves，38 B
```

这里的最少限定为单元素移动、不允许整体 replace；其值为 `n−LIS`。若允许整根 replace，任何改变都能用一条操作，单纯追求最少操作数没有足够意义。

[imara 官方源码](https://docs.rs/imara-diff/0.2.0/src/imara_diff/lib.rs.html#167-221) 明确 Histogram 是启发式。MyersMinimal 保证的也是序列增删最短，不是所有 JSON 的最小字节 delta。

#### 5.2 RFC 导出会重复移动同一项

```text
[0,1,2,3] → [1,3,0,2]
native：2 moves
RFC：  3 moves；独立应用器验证两步即可完成
```

5,906 对中有 3,072 对 RFC moves 多于 native entries。前后半区交换：

| 数组长度 | 最少 / native moves | RFC 导出 moves |
|---:|---:|---:|
| 20 | 10 | 41 |
| 50 | 25 | 184 |
| 100 | 50 | 576 |
| 500 | 250 | 9,075 |

这些结果都正确还原，但操作体积被放大。[export.rs:65](src/export.rs#L65) 的前缀修复会重复移动，并用线性查找、Vec remove/insert 模拟。n 项、p 条输出 move 的这一段约 O(n·p)；p 也会放大，不能简单把整个 exporter 写作 O(n²)。

独立 Python 的“从右侧目标顺序安排移动”候选在 5,906 个纯排列上正确且不增加移动；它尚未覆盖混合增删，也尚未实现到 Rust。唯一 token/身份的 LIS 路径也是可研究的内部改进，不需要先增加公共配置。

### 6. 实测性能

#### 6.1 既有原生 delta：对 JS 的优势有明确适用范围

Apple M2 Max，Rust 1.97.0，Node 24.14.0；parse→diff→serialize 完整 native delta，单位 ms。

| 输入 | 本库 | JS jsondiffpatch | 本次观察 |
|---|---:|---:|---|
| 小配置单字段 | 0.0120 | 0.0063 | 本库较慢 |
| 2,000 字符串单项移到头部 | 0.4352 | 43.3850 | 本库约快 99.7 倍，双方 27 B |
| 2,000 ID 对象移动加修改 | 2.2307 | 80.4370 | 本库约快 36.1 倍，双方 74 B |
| 真实元数据 100 项的受控变化 | 0.7749 | 1.0088 | 本库较快 |
| 长 ASCII 文本 | 1.0407 | 0.3013 | 本库约慢 3.45 倍 |
| 长 Unicode 文本 | 0.9545 | 0.3148 | 本库约慢 3.03 倍 |

16 对实际可比较输入，本库快 9 对、慢 7 对；这不是业务加权排名。纯文本 diff 的差距更大，ASCII 约慢 63 倍，Unicode 约慢 37 倍，pipeline 中解析/序列化会掩盖这部分差距。[原始结果](results/baseline-20260930/comparison.json)

四项 JS 20k 大矩阵负载是预算跳过，未实测 OOM/超时。RSS 是整个进程峰值，不是库分配量。Criterion 38 项是本库自身统计，不是竞品横向第一证据。

#### 6.2 本轮补测标准输出全过程

对同一组冻结输入，链接当前 release 库，直接比较 typed `json_patch::diff` 与 `本库 diff→to_json_patch`，都解析两份文档并序列化 RFC Patch。20 次预热、5 个约至少 20 ms 的计时批次、取中位数；串行执行，27 个测量全部通过各自正确性检查。

| 输入 | 本库→RFC 时间 / 字节 / 条数 | json-patch 时间 / 字节 / 条数 |
|---|---|---|
| 小配置 | 0.0155 ms / 53 B / 1 | 0.0080 ms / 53 B / 1 |
| 2k 头部插入 | 0.5245 ms / 40 B / 1 | 0.4885 ms / 112,933 B / 2,001 |
| 2k 单项移动 | 0.5391 ms / 42 B / 1 | 0.4888 ms / 112,891 B / 2,000 |
| 2k 全部不同 | 4.5162 ms / 165,781 B / 4,000 | 0.4870 ms / 112,891 B / 2,000 |
| 2k ID 对象移动加修改 | 3.5534 ms / 116 B / 2 | 3.0655 ms / 479,232 B / 8,000 |
| 元数据 100 项 | 1.1249 ms / 150 B / 2 | 1.2236 ms / 155,518 B / 1,832 |
| 长 ASCII | 1.4508 ms / 180,044 B / 1 | 0.2497 ms / 180,044 B / 1 |
| 100 项半区交换 | 0.3832 ms / 22,982 B / 576 | 0.0161 ms / 4,081 B / 100 |
| 500 项半区交换 | 6.7884 ms / 380,543 B / 9,075 | 0.0769 ms / 21,281 B / 500 |

这证明本库在标准输出下常以 CPU 换取小补丁，但当前 exporter 也会出现速度和体积同时变差。`to_json_patch` 包含当前公开 API 的基线校验/重建成本；不能当成隔离测量的算法核心。

native 的 500 项半区交换只有 0.1489 ms、4,400 B，说明瓶颈主要在转换步骤。新测量是一次进程内有限样本，不能作为所有平台的稳定倍率；没有测进程 RSS或每库分配。

旧原型评估将部分 typed Patch 先转 Value 才计 diff，预热也不同；且部分正确性失败的行仍有时间记录，必须排除。这些数字不能拼进当前库的纯 diff 排名。[旧 harness](../jsondiffpatch-evaluation/src/main.rs)

### 7. 互通和测试证据的真实边界

#### 新增有限验证

- 数组分支：5,946 组重排、重复身份和嵌套编辑；10,000 组独立模型生成的合法外部 delta，全部 forward/reverse/RFC 验证通过。
- 算法分支：5,906 小排列，Rust 与 JS 往返均通过；与上述数组语料部分重叠，不能把它们相加成独立覆盖量。
- 文本分支：240 组 ASCII/emoji/组合字符/NUL/CRLF/URI字符与多项编辑，四条 native/JS 往返路径全部通过。
- 标准最小 probe：确认数值 test 与根 move 的问题；独立 debug probe 确认畸形文本 panic。
- 27 个新增性能测量均通过正向检查；native 输出还验证了原 delta reverse。

#### 不可信 delta 测试有空白

[properties.rs:21](tests/properties.rs#L21) 对象键只生成 `[a-z~/]{0,6}`。复用它的 untrusted_delta suite 生成不了 `_t`、`_0`、数字目标键，因此不会探索真正的数组协议。随机字符串也很难进入有效 DMP header 深层。

8,192 个固定 seed 的属性案例是可靠回归证据，不是持续的新随机探索、覆盖率 fuzz 或错误输入完备性证明。应补协议感知的生成/变异，尤其是坐标归一化、巨大索引和冲突位置。

#### JS 自逆向问题真实，但不能推导总体失败率

独立克隆所有输入重跑 JS 0.7.6：基础 373 对全部通过，600 组混合身份数组有 501 个逆向错误，100 文本有 6 个，合计 507。唯一 ID 的三项逆序并编辑也能复现，不能全部归因于重复身份。

该合成语料刻意集中在复杂组合，不能称“上游实际有约 47% 的失败率”。

此外，此前 `JS delta→Rust 1073/1073` 是应用 JS **正向** delta，再用 **Rust 自己的 reverse**。它不表示任意 JS inverse 都能被 Rust 应用：95 对 JS 自己能恢复的文本 inverse 被本库拒绝，主要与严格位置/上下文及 JS 头部宽松行为有关。支持协议互通不等于 JS 所有行为完全兼容。

#### 发布成熟度仍有限

既有 test/Clippy/fmt/MSRV/package 记录对应当前源码。MSRV 是 `check --lib --locked`，不是所有 dev 测试在 1.85 上运行；当前没有 Linux/Windows 持续验证、协议定向 fuzz 或真实生产使用证据。未发布本身不是 bug。

### 8. 真实需求与定位

| 使用方 | 源码证据 | 能证明的需求 |
|---|---|---|
| [kube-rs](https://github.com/kube-rs/kube/blob/d22d799cce551d07e5301ee4ca994691d9ae7c29/kube-core/src/admission.rs#L363) | AdmissionResponse 接受 json_patch::Patch | 标准协议兼容优先，不必使用 native undo |
| [Tauri](https://github.com/tauri-apps/tauri/blob/f04089769d8c6bcd1b8d31870e26390019715ebc/crates/tauri-codegen/src/lib.rs#L85) | 用 json_patch::merge 合并配置 | Merge Patch 需求，不是身份数组移动需求 |
| [Redux DevTools](https://github.com/reduxjs/redux-devtools/blob/89ae6ee57c6879c0da39d1f03d6ad8de029c5390/packages/redux-devtools-inspector-monitor/src/createDiffPatcher.ts) | 身份、过滤、结构展示；关闭 detectMove | 身份匹配需求真实，但不能以 move 倍率推导采用 |
| [Jotai DevTools](https://github.com/jotaijs/jotai-devtools/blob/9208b45732ef18091205ceba30ccecc58036fc0a/src/DevTools/Extension/components/Shell/components/TimeTravel/utils/create-diff-patcher.ts) | 类似 DevTools 实现，同样关闭 move | 同源使用证据，不是完全独立算法选择 |
| [json-history](https://github.com/lustan3216/json-history/blob/b3e8e7dcc62c3724b30ec1d8c0c8f953643c6fe3/src/index.js#L240) | 存 delta，undo/redo | 直接可逆需求，但活动较旧 |
| [json-sync](https://github.com/sueddeutsche/json-sync/blob/cc14424c6d675bfbabd5aba517f9c431781abc95/lib/diffpatch.js#L13) | _id、move、增量同步 | 接近本库用途，但仓库已归档 |

[arrays-by-hash](https://github.com/schnerd/jsondiffpatch-arrays-by-hash#why) 说明 ID 在 diff 阶段匹配、delta 在 patch 阶段仍用索引，两个同基线 patch 不能任意叠加。版本、基线和冲突处理由上层负责，不能把本库宣传为协作合并引擎。

### 9. 建议改进顺序

| 顺序 | 必要工作 | 可审查验收 |
|---|---|---|
| 1 | 修复标准数值 test、文本归一化溢出，确认根 self-move；说明 move 基线契约 | 新最小反例返回正确结果或明确 Error，不 panic、不返回非法逆补丁 |
| 2 | 修 RFC move 排序，研究唯一身份/token 的 LIS；优化已实测的文本弱项 | 半区交换不再放大到数千操作；保留混合增删改往返；计量 RFC 时间与体积 |
| 3 | 定向协议测试、新 seed、必要跨平台与 MSRV 验证 | 真实数组/DMP坏输入覆盖，现有回归保持通过 |
| 4 | 同合同竞品对照与真实使用方负载 | native 与 RFC 分开；typed 输出；只给正确样本计时；记录操作数、字节、CPU与基线需求 |

是否增加 Merge Patch、无序/容差、模糊文本、插件或绑定，要由具体使用需求决定。先完成这些质量改进，才能支持“这个明确场景下很强”；不能通过不断加功能得到一个有证据的“全球最好”。

### 最终判断

- **已有价值：** 可逆 JSON 和大规模身份数组，尤其需要 JS native delta 互通的 Rust 应用。
- **不是功能最全：** JSON 范围内就存在已核查的缺项，无需拿 Python对象或 CRDT 来凑差距。
- **不是全面最快：** 小对象、文本、全部不同数组和当前 RFC 导出都有明确反例。
- **尚不能称综合最好：** 正确性边界需要修复，持续验证与实际采用尚未建立。
- **值得继续做：** 已有窄场景性能和功能组合差异；最先投入正确性、补丁质量和真实需求验证。
