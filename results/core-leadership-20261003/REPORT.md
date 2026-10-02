# Palim：核心能力与性能改进，2026-10-03

## 结论

本轮按三个独立任务研究竞品、性能和能力，并在 `be76218` 基线上实现改进。目标是 Rust JSON diff/patch 基础库；可比范围为可逆 JSON delta、数组身份与移动、文本增量、RFC 6902、RFC 7396、比较报告和错误合同。比较 Rust json-patch 4.2.0、JS jsondiffpatch 0.7.6、Go wI2L/jsondiff v0.7.1 的对应接口。

**已有明确的场景优势，但不能宣称全功能、全输入、全资源指标都领先。** 最新实测仍有两个反例：原生 Unicode 多处编辑和小对象的完整解析流程慢于 JS；分配次数、峰值内存与跨机器 P99 尚未完成排名。公开这些差距，逐项消除，才能形成可信的领先结论。

本轮新增两个 API，优化四处已证开销，保留原有错误与回调合同。没有新依赖、公共配置或另一套 JSON 表示。主工作区另有用户会话，实施与测试先在独立 worktree 完成。

## 已实现的能力

### 只读 RFC Test

新增 `test_json_patch(&Value, &Patch) -> Result<(), Error>`。只拥有共享借用时无需复制文档即可执行 Test；源深度、期望值深度、Pointer 和数学数值相等仍按现有合同检查。按序执行，遇到修改操作在其当前位置报错，不隐藏更早的 Test 失败。

这是 Rust 用户明确提出的需求：[json-patch #52](https://github.com/idubrov/json-patch/issues/52)。json-patch 的内部 Test 本来就不需要修改数据；成本来自它的公开接口要求 `&mut Value`，使只有 `&Value` 的调用者需要先克隆。不能把调用者的克隆说成其 Test 算法必然克隆。

### 保存带检查的逆补丁

新增 `invert_json_patch_guarded(&Value, &Patch) -> Result<Patch, Error>`。复用原逆转步骤和 guard，消费已生成的最终 shadow，避免再复制整份文档。原来的 `invert_json_patch` 合同保持。

用 [jsondiffpatch #254](https://github.com/benjamine/jsondiffpatch/issues/254) 的数组顺序漂移案例验证：新 API 拒绝在错误顺序上撤销；旧无 guard 的标准逆补丁仍可能删除错误位置。本轮解决的是**检测并拒绝错误基线**，没有实现按 ID 自动重定位或冲突合并。身份匹配与 RFC 索引应用的边界需要保留。

检查范围是撤销涉及的值和必要容器，并不认证整份文档。现有对象字段的 Replace 可容许其他兄弟字段漂移；恢复缺失键或数组插入需要整个父容器快照，可能拒绝无关兄弟字段的变化，也会增加体积。

Go 当前 `Patch.Invert` 已存在，不再把历史逆转需求当作当前竞品缺失。官方版本和实现见 [v0.7.1](https://github.com/wI2L/jsondiff/releases/tag/v0.7.1)、[patch.go](https://github.com/wI2L/jsondiff/blob/cc662875ff5888a22ab73d512826040d81b67811/patch.go)。

## 已实现的性能优化

1. **RFC 直接替换文本。** RFC 导出只能把文本 delta 转为整值 Replace，因此标准生成不再先构造随后丢弃的文本增量。native 文本增量不受影响；过滤投影、身份匹配、回调顺序、深度限制与后续 factorize/rationalize 保留。
2. **默认全异 primitive 根数组直接生成标准操作。** factorize 开启时保留原来的倒序 Remove、顺序 Add，继续让优化器选择 Copy 和父节点替换；plain 路径保留已有位置 Replace。容器、过滤、定制 matcher 与 guards 使用原路径。`object_hash` 原来仅对对象/数组调用，没有跳过原本发生的 primitive 回调。
3. **Rationalize 减少重复扫描。** 初始 patch 未改变时，延迟建立祖先到操作的索引；接受第一个父节点替换后恢复扫描，避免重复建索引。保留候选顺序、Move 跨边界判断、字节预算和真正的候选应用验证。
4. **减少临时空间。** native 文本只累计到实际 hunk 起点的 UTF-16 坐标，不再为每个字符存一个 usize；标准 payload 深度遍历只把容器入队。字符序列和行匹配仍存在；没有宣称已测出整体峰值内存的改善幅度。

### 已解析输入的隔离优化测量

生成和输出析构计时，解析、序列化与往返检查不计时。基线与原型独立构建，3 次预热、至少 5 ms 校准、7 批样本、每版 3 个独立进程，顺序轮换。下表为进程中位数的中位数。集成树另通过 2,984 项逐字节等价检查。

| 默认 RFC 场景 | 基线 ms | 改进 ms | 提速 |
|---|---:|---:|---:|
| 全异数组 2,000 项 | 4.340 | 2.366 | 1.83× |
| 重复身份数组 2,000 项 | 23.847 | 7.908 | 3.02× |
| 小配置编辑 | 0.004017 | 0.004001 | 基本不变 |
| 唯一项旋转 2,000 项 | 0.329 | 0.332 | 基本不变 |
| 大片稳定内容 1 MiB | 0.02758 | 0.02767 | 基本不变 |
| 400 个可压缩父节点 | 174.501 | 177.531 | 约慢 1.7% |

共享机器有其他用户会话，小幅差距不能确认真实退化。400 父节点负载的整文档克隆与候选验证开销仍未解决，不能只报前两项收益。重复身份的 rationalize 阶段（包含传入 patch 的克隆）约 19.885 → 4.057 ms。

文本独立原型 30 个进程测量：Unicode 三处编辑的已解析 diff 1.589 → 1.231 ms，完整流程 1.930 → 1.555 ms；全异 Unicode diff 3.148 → 2.916 ms。单处文本基本不变。300 个固定 Unicode/NUL/CRLF/多处编辑案例及 5 个正式长文本输入，完整输出字节相同。

## 最终代码的竞品重测

最终集成代码重新运行 **108 个独立进程**，每组 3 个进程。Rust 两个 runner 从集成树重新构建；JS/Go 使用已冻结版本、锁文件和输入。RFC 边界为解析两份 JSON → typed diff → 序列化补丁；native 边界为解析两份 → 可逆 delta → 序列化。验证在计时外。

72 项 RFC 测量中，Rust runner 通过独立 json-patch 应用；Go 的全部 36 份输出通过 fast-json-patch 3.1.1 独立应用。24 个 JS native 测量均验证 forward/inverse；Palim runner 验证 forward/reverse/unpatch。没有把 Go inverse 纳入本轮计时或验收。

| RFC 完整流程 ms | json-patch | Palim plain | Palim optimized | Go default | Go optimized | Go optimized + LCS |
|---|---:|---:|---:|---:|---:|---:|
| 小配置编辑 | 0.01021 | 0.01031 | 0.01117 | 0.01554 | 0.02377 | 0.02449 |
| 全异 2,000 项 | 0.462 | 0.476 | 2.508 | 1.505 | 14.313 | 39.526 |
| 长 ASCII 文本 | 0.241 | 0.250 | 0.311 | 2.436 | 2.597 | 2.599 |
| Unicode 三处编辑 | 0.434 | 0.443 | 0.521 | 3.870 | 4.034 | 4.033 |

全异数组 plain/default 为 112,891 B、2,000 Replace；optimized 为 34,038 B、1 root Replace。其他三个场景所有引擎体积分别为 53、180,044、230,060 B。不能把速度和体积不同的选项混排。Palim optimized 全异数组约快于 Go 同优化模式 5.71×；Palim plain 与 json-patch 很接近，未普遍领先。

Unicode RFC plain 的前轮基线为 2.114 ms，本轮 0.443 ms，消除了被丢弃的中间文本匹配；json-patch 本轮 0.434 ms，差约 2%，不能声称反超。

| Native 完整流程 ms | Palim | JS 配置 ID 回调 | JS ID 回调且 position=false | delta B |
|---|---:|---:|---:|---:|
| 小配置编辑 | 0.01013 | 0.00673 | 0.00635 | 28 |
| 唯一项旋转 2,000 项 | 0.447 | 40.977 | 41.002 | 27 |
| 全异 2,000 项 | 1.405 | 278.532 | 279.054 | 111,790 |
| Unicode 三处编辑 | 1.540 | 1.095 | 1.133 | 311 |

双方 native 均配置相同 ID 回调。JS 第一列没有显式配置 matchByPosition，并非无配置 create()；回调会避开 primitive 的位置匹配预扫描。不能拼入此前 GitHub #365 的无回调慢路径倍率。旋转与全异输入的优势不能代表所有数组。

小对象已解析 diff 是 Palim 0.002697 ms、JS 0.004825 ms，但完整流程的 parser/runtime 成本使 Palim 更慢。Unicode 已解析 diff 是 Palim 1.228 ms、JS 0.724 ms，说明该差距主要仍在文本匹配。阶段插桩推断行分组 interning/refinement 为主要剩余热点；这属于诊断依据，不是替代正式 release 计时。

### 新 API 的真实成本

Criterion：1 个进程、20 批样本、100 ms 预热、目标 300 ms；以下点估计不等同于跨机器稳定倍率。文档含约 1.06 MiB 未变字符串及 1,000 个整数，排除 parse/serialize。

| Test-only 调用边界 | 1 条 Test µs | 1,000 条 Test µs |
|---|---:|---:|
| Palim 只读借用 | 0.501 | 156.357 |
| Palim 借用并返回克隆 | 41.083 | 196.899 |
| json-patch 调用者先克隆 | 38.800 | 195.892 |
| json-patch 已有可变文档 | 0.163 | 150.146 |

只读 API 对大文档上的少量 Test 避免克隆非常有效；已有 `&mut Value` 的 json-patch 更快，而且省略 Palim 源深度检查。不能从前两行推出所有 Test 场景都更快。

2,000 次字段 Replace 的逆补丁生成：普通 2.674 ms，带检查 4.817 ms。guards 提供额外验证，存在明确成本；父容器快照的成本取决于输入，不作普遍常数倍承诺。

## 被否决的优化

- **每次压缩后重建祖先索引**：400 个父节点约 183 → 267 ms，退步约 46%；未采用。最终只在初始 patch 未变时建立索引。
- **自定义 matcher 优先完全相同值**：1,482 组完美匹配图与排列中，29 组估计 Move+Replace 质量更差；实际反例从 2 Replace 变为 1 Move+3 Replace。正确往返并不等于更优补丁，未采用。
- 不通过更粗文本替换、丢旧值或取消真正的候选验证来制造速度收益。前两者会改变质量/可逆合同，后一项需要另行证明合法的局部依赖边界。

## 如何把“全面领先”变成可验收的工作

采用当前序列引擎，适配已存在的 RFC/guard 功能，构建两个小 API；拒绝本轮两种有反例的捷径。下一步按下面的可比合同补齐证据与差距，避免先扩出新平台。

| 验收组 | 必须一起报告 | 当前状态与下一步 |
|---|---|---|
| 小对象 / 大稳定子树 | 已解析与完整流程、分配、相同原子失败合同 | 小对象完整流程仍落后；先量 parser 和结构克隆，避免直接重写核心算法 |
| 唯一项数组 prepend/rotate/reverse | 同配对、Move 数、补丁 B、生成/应用 | 旋转已领先；其他增长曲线仍需冻结同一资源上限 |
| 全异数组 | plain 与 optimized 分列、相同目标体积 | 默认冗余构造已移除；不能要求更小与更快在所有输入同时成立 |
| 重复词汇与重复 ID | 20k/40k 增长、匹配质量、候选预算 | 重复 ID 压缩已提升；自定义匹配的全局最优仍未保证 |
| Unicode / 多处文本 | UTF-16、JS 往返、体积、生成/应用 | 行分组匹配仍落后；优先验证借用 UTF-8 行 token，保留原 hunk/字节质量 |
| 大量独立父节点压缩 | 400 父节点对照、跨界 Move/Copy、字节等价 | 下一热点是候选整文档 clone+apply；只有证明稳定对象路径和局部依赖才能缩小验证范围 |
| Test-only / inverse | 借用与已拥有输入分列、漂移拒绝、guard 体积 | 两个 API 已补齐；guards 的父快照范围和成本已公开 |
| 资源与交付 | 分配、峰值 RSS、p50/p95、MSRV、打包消费者、跨平台 | 本轮 MSRV/消费者通过；内存与跨机器尾延迟尚未排名，版本未重新发布 |

完全最小编辑、最小字节、最少内存和最低延迟可能冲突。领先应声明指定的功能/输入/合同与资源范围；本轮没有证明“所有竞品的全部功能都超越”。浏览器展示、语言绑定和按 ID 冲突合并是额外产品能力，不能冒充核心 JSON 库已经具备。

## 已完成的检查

- debug/release **各 157 项测试**，fmt、Clippy `-D warnings`、Rust 1.85 库检查通过。
- 373 对保存输入 × 8 种 RFC 选项 = **2,984 项**，最终集成树输出与 be76218 逐字节相同，forward/inverse 通过。
- JS 互通 **1,073 项**，Rust forward、Rust inverse、Rust RFC、JS delta 经 Rust 往返全部通过。JS 自己的逆向仅 566 项成功，继续保留既有分类；不表示所有上游 JS inverse 可严格应用。
- fuzz 含新增只读及 guarded inverse，固定 seed、max_len 2048，**61 秒、380,223 次**，无崩溃；短时 fuzz 不是无缺陷证明。
- `cargo package --allow-dirty --locked` 验证通过；无初始 lockfile 的 Rust 1.85 消费者运行解包产物，通过两个新 API、漂移拒绝、精确数字及长 Unicode 往返。依赖离线从已缓存 registry 解析。
- 两组新增 Criterion 实际运行；最终竞品 108 个进程样本完成。
- 独立只读 API 审查无阻断。没有重新运行远程三平台 CI，没有推送或重新发布 crate。

## 证据与复现

- [原始样本、汇总、环境和源/二进制哈希](measurements.json)
- [实际检查输出、消费者、等价验证](checks.json)
- [冻结 runner、源码、输入、lockfile、原型及拒绝证据](evidence.zip)

证据包中的 final-runners 使用记录的本地路径，迁移环境时需要修改路径常量；不要换输入、选项或依赖版本后沿用本轮排名。分析阶段的私有 profiling hooks 未进入生产 API；正式竞品使用现有两个 examples。新增 API 复现：

```sh
cargo bench --bench core --locked -- standard/test-only --noplot
cargo bench --bench core --locked -- risk/standard-apply/invert --noplot
```

所有结果来自同一台 macOS；其他用户会话的系统负载不可完全控制。没有混合已解析输入、完整流程、预拥有数据和借用数据的倍率。
