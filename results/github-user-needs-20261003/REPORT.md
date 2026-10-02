# GitHub 用户问题与 Palim 的实际适用范围

核查日期：2026-10-02 至 2026-10-03。Palim 源码基线：`6101f6ee9fdf03ce12fe5d698f005b3a3f6654f5`。生产代码未修改。

## 结论

存在可以用 Palim 解决的真实问题，最明确的是：**可逆历史记录、移动并修改后的撤销、补丁数据不被后续操作修改，以及可配置的 JSON 比较与投影过滤。** 部分能力已在用户的原始案例上验证。

当前不能把所有 issue 都算作竞争优势。上游已经修复的问题、配置就能解决的问题、业务身份无法推断的问题和不同语言的集成成本，需要分别看待。没有向维护者发消息，也没有验证任何用户已经采用 Palim。

## 调查与验证范围

- 从 GitHub API 获取 JS jsondiffpatch 275 条、Go jsondiff 28 条、Rust json-patch 24 条 issue 元数据，包含全部列表分页并排除 PR；另外获取 DeepDiff 28 条和 .NET SystemTextJson.JsonDiffPatch 31 条，后两者仅覆盖最新 100 个 issue/PR 列表项中的 issue。合计 386 条标题/元数据，不表示逐条审核了所有讨论。
- 筛选并获取 42 条 issue 正文和评论；重点把 25 个 issue 转为 29 个复现或对照场景。输入来源明确标记为原始数据、JSON 转换或代表性案例。
- 实际编译 release 临时消费者运行 Palim 公共 API；18 个 native 场景验证前向、撤销、plain RFC、独立 RFC 逆向和 optimized RFC。另外运行序列补丁、标准应用、比较报告与换序基线检查。4 个 standard 场景中 wildcard 是明确拒绝的边界用例。
- 运行 npm 发布的 `jsondiffpatch 0.7.6` 和 `fast-json-patch 3.1.1`。查询 npm 后确认 0.7.6 为当时 latest。17 个生成的 JS 正向 delta 导入 Palim 后，应用并用 Rust reverse 撤销全部成功。不是 29 个场景全都被所有语言的库运行。
- 未运行 Go、Python 或 .NET 库的消费者；这些语言的结论限于 GitHub 原文/评论、相关 API 源码与 Palim 的对应实测。不能把复现于 JS 的结果算作它们的失败。

[筛选元数据与评论链接](issues.json)、[带来源标签的冻结输入](fixtures.json)、[实际运行结果与版本/源码哈希](verification.json)、[复现程序](verify.py)。

## 1. 当前 JS 发布版仍能复现，Palim 在相同输入上成功

| 用户问题 | 当前 JS 0.7.6 的观察 | Palim 的实测结果 | 适用场景 |
|---|---|---|---|
| [#438 文本逆补丁失败](https://github.com/benjamine/jsondiffpatch/issues/438) | 原始长文本前向成功，unpatch 报 text patch failed | 原生生成和撤销成功；issue 提供的正向 delta 也能导入并用 Rust reverse 撤销。JS 自己生成的 inverse 精确应用失败，显式 fuzzy 成功 | 编辑历史、文档版本、服务端恢复 |
| [#275 换位同时修改字段，撤销结果错误](https://github.com/benjamine/jsondiffpatch/issues/275) | 原始四个对象案例前向成功，撤销结果不等于原文 | name 身份配对后前向/撤销和标准导出成功；导入 JS 正向 delta 后 Rust reverse 也成功 | 对象列表、排序并编辑 |
| [#295 重排对象并修改 zIndex 后撤销错误](https://github.com/benjamine/jsondiffpatch/issues/295) | 原始九个对象案例前向成功，撤销结果错误 | id 身份配对后往返成功；JS 正向 delta 可被 Rust 正确撤销 | 可视化编辑器、场景数据、列表历史 |
| [#377 应用后续补丁修改了旧补丁](https://github.com/benjamine/jsondiffpatch/issues/377) | 原始 stages 两步操作后，p1 内容发生改变 | 两步生成/应用/撤销成功；p1、p2 均未改变 | 保存补丁、重放审计、undo 栈 |
| [#420 相同嵌套数组产生多余 diff](https://github.com/benjamine/jsondiffpatch/issues/420) | 两份相同结构仍产生 delta，标准导出两项；应用结果正确 | native None、RFC 零操作 | 配置监听、变化检测、避免无效保存 |

这是固定案例上的区别，不证明其所有同类输入都优于 JS。#275/#295 的 JS inverse 直接导入 Palim 仍失败，因为 inverse 已把旧值/索引关系写错；解决路径是导入完整正向 delta，使用 Palim 自己的 reverse/unpatch。不能把错误 inverse 宣称为可自动修复。

### 使用成本

Palim 当前提供 Rust crate。JS 前端、Python 或 .NET 用户无法直接替换 import 后使用；需要 Rust 服务端、进程接口或语言绑定。上述 issue 提供需求证据，不提供现成迁移渠道，也不表示这些用户愿意迁移。

## 2. 通过已有配置可以满足的需求

| 需求与来源 | 已验证的使用方式 | 边界 |
|---|---|---|
| [Go #15 新增对象内部的忽略字段仍进入补丁](https://github.com/wI2L/jsondiff/issues/15) | native `node_filter` 排除 `/nested/exclude`，结果只包含 include，投影结果可撤销 | 改用 node_filter；不能把父节点操作简单删除，否则会连 include 一起丢失 |
| [DeepDiff #562 只比较共有字段](https://github.com/seperman/deepdiff/issues/562) | `CompareOptions.node_filter` 接受两侧都存在的节点；嵌套新增/缺失键不产生报告差异 | 代表性 JSON 案例通过。CompareReport 是报告，不是宽松匹配后的可逆补丁 |
| [.NET #52 比较时忽略 actual 的额外字段](https://github.com/weichch/system-text-json-jsondiffpatch/issues/52) | 以 actual 为 left、expected 为 right，过滤 right 缺失的节点；嵌套对象的子集比较通过 | 验证覆盖对象子树；没有定义任意数组的子序列/子集规则 |
| [.NET #39 忽略数组顺序](https://github.com/weichch/system-text-json-jsondiffpatch/issues/39) | `CompareOptions.unordered` 对选定路径启用；嵌套乱序相等，重复数量减少仍产生差异 | 数组多重集语义，仅用于报告，不丢弃 native patch 的目标顺序 |
| [JS #424 字符串变化希望显示为修改](https://github.com/benjamine/jsondiffpatch/issues/424) | 默认是 delete/add；调用者提供本例的去除数字后缀 matcher 后，生成一项 Replace，正确往返 | 默认行为并未自动满足要求。matcher 是业务规则，不能推广为所有字符串的身份规则 |
| [DeepDiff #633 排除路径仍调用 custom operator](https://github.com/seperman/deepdiff/issues/633) | JSON 投影案例里先 node_filter 排除 `/b`，custom_equal 回调路径仅为根、/a、/a/v | 原报告是 Python 自定义对象；Palim 只接受 JSON，不能宣称支持任意 Python 类 |
| [Rust #7 希望撤销 RFC 补丁](https://github.com/idubrov/json-patch/issues/7) | `invert_json_patch(baseline, patch)` 对删除操作生成可保存的逆补丁，往返通过 | 标准逆补丁需要原基线；native 完整 delta 的 reverse 无需基线 |

### 过滤的实际缺口

[.NET #48](https://github.com/weichch/system-text-json-jsondiffpatch/issues/48) 报告 root null 与 object 切换时 PropertyFilter 不生效。Palim 的 `property_filter` 在同一输入上也不会进入新对象后代，因此不能把这一需求写成已完全解决。

改用 `node_filter`，null → object 时成功只保留 CompareMe。object → null 是授权的父节点原子替换，仍生成 null；不会为了保留 IgnoreMe 自动制造一个对象。因此只有前一方向的投影需求已经满足。需要调用者明确父节点的选择规则。

## 3. 性能需求：原始 2.2 万项输入

[JS #365](https://github.com/benjamine/jsondiffpatch/issues/365) 的用户报告大数组 diff 很慢，提供了 [独立复现仓库](https://github.com/soumilbaldota/jsondiffpatch-reprod)。本次下载其中两个 JSON 文件，按原 example.js 的方向调用 `diff(new.json, old.json)`：21,936 项到 21,970 项。输入主要为字符串 ID。

每引擎三个独立进程，顺序轮换；每进程先完成一次生成，再采三次已解析输入的 diff 耗时；表中为进程中位数的中位数。应用、撤销及标准导出在计时外验证；生成和 runtime 成本不同，不能当成端到端或跨平台排名。

| 选项 | 已解析输入 diff 中位数 | 标准补丁条数 | 结论 |
|---|---:|---:|---|
| Palim 默认 | 3.841 ms | 34 | 默认无需额外绕过 JS 的数组匹配预检查 |
| JS 0.7.6 默认 | 1844.902 ms | 34 | 同一公开输入重现慢路径 |
| JS 0.7.6，`matchByPosition:false` | 3.401 ms | 34 | issue 评论中的配置解决慢路径，本次略快于 Palim |

默认耗时比约 480×，属于具体数据和默认配置；不能把它写成 Palim 全面快 480×。JS 用户已有低成本配置方案，其配置后的实现本次约快 13%。这个 issue 证明默认体验和避免额外二次扫描有价值，单凭它不足以说服用户迁移。

JS 运行限制记录为 1 GiB V8 old-space、30 秒超时；本轮均正常完成，没有 OOM 或超时。Palim 单个输入执行，未把独立进程次数累加为功能覆盖率。未测浏览器、内存峰值、业务网络与序列化。

## 4. 当前不能直接满足的真实需求

| 需求 | 证据与 Palim 的实际边界 | 是否值得新增能力 |
|---|---|---|
| [JS #254](https://github.com/benjamine/jsondiffpatch/issues/254)、[#353](https://github.com/benjamine/jsondiffpatch/issues/353)：重新排序后的基线仍按对象 ID 打补丁 | 原始 code 案例中 Palim native unpatch 拒绝错误位置的旧值，没有自动定位 code=3。匹配回调只用于生成，delta 应用仍按索引 | 需求明确，但需要定义稳定 ID、冲突、重复 ID 和基线漂移合同；是显著的新能力，不能当一次小修复加入 |
| [JS #436](https://github.com/benjamine/jsondiffpatch/issues/436)：OOXML 深层节点没有稳定 ID，全内容 hash 导致 diff 扩大 | issue 只给出 delta，没有完整左右文档，无法原样复现。路径敏感 matcher 可表达业务规则，但内容变化后无法凭空知道同一段落身份 | 先收集完整输入与可用身份规则。不能宣称更换 Rust 引擎自然解决 |
| [Go #38](https://github.com/wI2L/jsondiff/issues/38)：新增整个对象也要叶子级事件 | 原始目标在 Palim plain RFC 中仍是一项 Add `/preferences`。生成器没有 leaf event 模式；从空对象直接 Add 不存在父路径的叶子也不是合法 RFC 应用 | 对报告/审计事件有用途，先确认输出消费者。与最小补丁优化目标有取舍 |
| [Rust #52](https://github.com/idubrov/json-patch/issues/52)：只读 Test 不复制整个文档 | Palim 可以接受 `&Value` 执行 Test，但 `apply_json_patch` 仍克隆后返回 Value。没有零拷贝只读 test-only API | 很适合底层库的小能力候选，但需先测真实 test-only 负载收益 |
| [Rust #43](https://github.com/idubrov/json-patch/issues/43)、[#42](https://github.com/idubrov/json-patch/issues/42)：JSONPath、通配修改 | 原始 `/?/name` 被 Palim 明确拒绝，RFC Pointer 没有 wildcard 合同 | 如要支持应有独立查询/选择边界；不改写标准 API 的含义 |
| [JS #381](https://github.com/benjamine/jsondiffpatch/issues/381)：RFC 补丁转 native 格式供 HTML formatter 使用 | Palim 可在有基线时先 apply RFC，再 diff；四个标准场景验证该组合可行。没有无基线直接转换接口或 HTML formatter | 满足 Rust 数据流程的部分需求；可视化目标还需要 formatter |
| [JS #439](https://github.com/benjamine/jsondiffpatch/issues/439)：CLI patch 子命令 | Palim 只有开发用 example runner，没有正式用户 CLI | 工具分发需求，不能宣称 crate 已提供该产品 |

### 基线安全特别说明

#254 的 native old-value 检查能避免本例错删。测试同时观察到：带 tests 的 RFC 正向补丁经 `invert_json_patch` 后，逆补丁不会自动继承正向 guards。把这个逆补丁应用在错误顺序上仍可接受并得到错误目标。因此不能把生成正向 guards 宣称为任意漂移基线上的安全 undo；逆向使用仍要求正确基线或另外生成所需检查。

## 5. 上游已修复或已具备，不能作为独有优势

- [JS #380](https://github.com/benjamine/jsondiffpatch/issues/380)：维护者说明在 0.7.2 修复多 Move 标准导出；本次 0.7.6 和 Palim 都成功应用原始反转案例。
- [JS #371](https://github.com/benjamine/jsondiffpatch/issues/371)：Pointer 转义 issue 已关闭；本次 slash/tilde 控制案例两者标准应用均成功。
- [Go #34](https://github.com/wI2L/jsondiff/issues/34)：当前 [patch.go](https://github.com/wI2L/jsondiff/blob/cc662875ff5888a22ab73d512826040d81b67811/patch.go) 已有 `Patch.Invert`。不能沿用历史缺失结论。Go #35/#28 应用请求的关闭原因则是包边界，维护者建议 evanphx/json-patch。
- [DeepDiff #540](https://github.com/seperman/deepdiff/issues/540)：维护者注明修复已随 8.5.0 发布。Palim 对原始 JSON 可表示的嵌套移动案例成功，不表示现在独有。
- [Rust #37](https://github.com/idubrov/json-patch/issues/37)：维护者说明对象变数组的问题在 1.3.0 修复；Palim 原始输入也成功，应当计为正常正确性。
- [Go #45](https://github.com/wI2L/jsondiff/issues/45)：issue 已关闭，Palim 原始 rename 案例的 factorize 成功；本轮未运行 Go 当前发布版，不能据此宣称当前 Go 仍会丢操作。
- [Go #41](https://github.com/wI2L/jsondiff/issues/41)：自定义相等请求最终 not-planned，但提问者也表示当前 Factorize/Rationalize/LCS 已满足其部分需求。Palim callback 提供更显式控制，不表示所有缺少 callback 的库都无法匹配变化对象。

## 6. 根据需求证据的建议

1. **优先展示已经实测的可靠 undo 与补丁隔离。** 在 README/示例中使用可公开复现的 #438、#275/#295、#377，说明正向 JS delta → Rust reverse 的迁移路径。它们无需新增核心架构。
2. **把比较与过滤写成少量应用示例。** 共有字段、expected 子树、无序多重集、过滤新增对象，是当前 API 已能解决的明确需求；例子需清楚区分 CompareReport 与 patch。
3. **如继续补一个小型底层能力，先评估 test-only 只读校验。** #52 指向明确的克隆成本，扩展范围比同步冲突引擎小。当前只是候选，没有测完性能或实现。
4. **按 ID 在漂移基线上应用是另一项产品决策。** #254/#353 提供需求，但应先取得身份/冲突合同和实际输入，再决定构建或采用。没有在本轮扩展 delta 协议。
5. **语言绑定决定 JS/Python 用户能否采用。** 当前事实是 Rust crate；性能或 bug 优势不会自动让原语言调用者完成迁移。

## 复现

需要 Rust、Python 3、Node 和安装了固定 npm 依赖的 tools 目录。脚本生成临时 Cargo 消费者并独立 release 构建，不修改库依赖，不使用生产 path override。消费者通过正常 path 指向待测 Palim，这是测试边界。

```sh
npm ci --prefix /path/to/palim/tools --ignore-scripts --no-audit --no-fund
python3 /path/to/palim/results/github-user-needs-20261003/verify.py \
  /path/to/palim \
  /path/to/palim/results/github-user-needs-20261003 \
  /path/to/palim/tools
```

构建使用 `cargo build --release --offline`，依赖需已缓存。再次运行会重写本目录 verification.json；如要保留旧结果，可将输出参数指向新目录。不同机器的时延可以变化，正确性观察应独立验收。实际脚本包含完整小型 Rust/JS 消费者，源码与输入哈希记录在结果中。
