# Palim 0.1.2：剩余瓶颈的定位与修复

日期：2026-10-04。基线为已发布 0.1.1 的 `6e762c1`。
本地最终源码绑定 RFC `710e0b9d…`、lib `808bdc12…`；全部 Rust 源码、fixture、runner、lock 与二进制 SHA-256 在 measurements.json 和证据包中。
这份报告是提交前的实际本地验收；随后提交绑定的 CI、crates.io checksum、registry 消费者与 tag 结果见 [v0.1.2 发布记录](https://github.com/majiayu000/palim/releases/tag/v0.1.2)。

## 已验证的瓶颈

对 800 个独立对象的 Replace-only 补丁，原实现 selection/prefix/compaction 分别访问
1,278,400 / 1,280,800 / 1,281,600 个操作槽，而被选中的总操作仅 3,200。
400→800 时全向量扫描次数约四倍。这是重复扫描的实测证据；profile 的计时包含插桩，不能当正式速度排名。

## 最小实现与合同

- 单个私有 Replace-only 分支沿用候选 fine-first 顺序。只为实际父候选建立借用路径到原始槽位的选择索引，冻结祖先写入标记；混合操作继续原算法。
- 正字节权重表示活动槽，零表示删除。初始真实 guarded prefix 差值保存重复/嵌套写入的中间 test 成本；接受替换只更新选中槽与精确总成本。及时释放废弃路径和值，最终压缩一次。
- 普通成本仍严格使用 selected-sum−1，guarded 成本包含真实 test 与逗号；沿用 bounded serialization 和严格小于收益判断。
- 普通 baseline proof 仍延迟到首次接受前，实际应用完整原补丁；失败时原补丁未修改并返回通用路径。guarded 初始真实 replay 与原错误保留。root/单活动操作候选仍真实验证。root 是最后候选，先释放索引与废弃槽再验证完整文档，避免峰值叠加。
- pointer 对普通 key 一次格式化并扫描 ASCII 字节；遇到 `~`、`/` 才执行既有 escape 顺序。父路径不重复编码，Display 仍只调用一次。无 API、依赖、feature、unsafe 或数值语义变更。

选择列表保持有效的理由：接受 A 后，后续父候选只可能包含 A 或与 A 不相交；Replace 不移位，没有 Copy/Move 来源读。过滤失活的原始槽位因此保持当前操作顺序与集合。严格祖先写入必须由更浅候选消除，该候选尚未访问。
这是源代码推导，另外用内部重复/交错/祖先写入、错误与预算边界执行对照验证。

## 正式测量

同一台 Apple Silicon macOS，release，3 个独立进程中位数再取中位数。
46 个输入/模式组合 × 两版本 × System timing/独立分配计数 × 3 进程，共 552 条记录。
每进程 3 warmup、至少 10 ms 校准（最多 2048 次）、9 批次，交错版本顺序。
所有记录完整输出 SHA 相同，计时外用独立 json-patch 执行器验证 forward/inverse。
普通模式计时已解析 Value 的生成及销毁；pipeline 明确计入解析、diff、序列化、临时值和输出销毁。

| 输入 | 模式 | 0.1.1 ms | 0.1.2 ms | 速度比 |
|---|---|---:|---:|---:|
| small-config-edit | native | 0.001941 | 0.001831 | 1.060× |
| small-config-edit | guarded | 0.007482 | 0.007475 | 1.001× |
| many-accepted-100 | optimized | 1.274641 | 0.731807 | 1.742× |
| many-accepted-100 | guarded | 1.396870 | 0.946060 | 1.477× |
| many-accepted-400 | optimized | 8.664875 | 2.994187 | 2.894× |
| many-accepted-400 | guarded | 8.653062 | 3.742521 | 2.312× |
| many-accepted-800 | optimized | 27.858125 | 5.861438 | 4.753× |
| many-accepted-800 | guarded | 26.436292 | 7.485896 | 3.531× |
| many-accepted-1600 | optimized | 85.556208 | 11.916250 | 7.180× |
| many-accepted-1600 | guarded | 84.113375 | 15.095500 | 5.572× |
| synthetic-after-first-accept-100 | optimized | 0.348454 | 0.247308 | 1.409× |
| synthetic-after-first-accept-100 | guarded | 0.564939 | 0.432594 | 1.306× |
| unicode-disjoint-24000 | native | 2.387870 | 2.357234 | 1.013× |
| unicode-three-edits | native | 0.428107 | 0.429637 | 0.996× |
| small-config-raw-pipeline | pipeline-native | 0.009496 | 0.009293 | 1.022× |
| small-config-retained-old-pipeline | retained-old-pipeline | 0.005704 | 0.005539 | 1.030× |

800 普通模式峰值额外请求 live 从 3,461,147→3,253,918 B（约降 6%），guarded 仍为 3,784,201 B。
小 native 的 allocation/reallocation 从 20→14 次。所有分配测量的最终 live delta 为零。
此计数是 allocator 请求字节与相对预存输入的峰值，**不是 RSS 或物理堆占用**。所有 46 行、每次原始 samples 与分配记录见 [measurements.json](measurements.json)。

完整小 JSON 双解析仅快约 2.2%，Unicode escaped pipeline 约 0.6%，没有显著解决 parser 成本。
若调用者已有旧 Value，可直接复用现有 `DiffPatcher::diff(&old, &new)`，避免重复解析旧文档。
retained-old 的 5.539 μs 对双解析 9.293 μs 是**不同调用边界**，不是同一边界的库内提速，也不是新 API 或 parser 的收益。
公开 Kubernetes/npm/TypeScript 文档只用冻结原文及受控编辑，不代表生产事件或用户采用。

## 回退与被否决的方案

首次索引保留全部 leaf/owned 字符串，800 普通/guarded 峰值分别涨约 18%/9%；缩到借用实际父候选后 guarded 持平、普通仍涨约 5%。
最终在 root 验证前释放索引和废弃槽，普通峰值转为约降 6%。三阶段原始测量保留为历史，未混入最终统计。

最初 RFC+pointer 组合在长 Unicode 全异文本稳定回退约 4–5%。统一版本号的四组合有界对照仍复现。
文本和 native 热函数的地址归一化指令内容相同；入口和调用地址发生变化，代码布局影响是**推断**，未测出 cache/分支硬件因果。
唯一新增候选将字符 contains 改为 ASCII byte scan：该检查只需要两个 ASCII 字节，UTF-8 多字节不可能误命中。
三次交错测量 2.433→2.327 ms，小 native 无回退，373+5+1 fixtures、12 callback 与 15 错误/深度/数值记录等价。
最终全矩阵长 Unicode 2.388→2.357 ms，没有复现约 5% 的回退；三处文本编辑仍约慢 0.4%，视为小幅波动，未声称每行都快。
没有加入对齐、inline、源码填充或平台特例来调整这个单例。

## 真实完成的检查

- fmt、diff whitespace、debug/release 各 170 项、Clippy all-targets `-D warnings`、Rust 1.85、release examples。
- 1,073 组 JS 必需互通全部通过；562 个合法 JS 自逆补丁由默认 fuzzy 还原，4 个畸形头部仍报错，507 个上游自逆失败单列。
- ASan fuzz 61 秒 / 357,393 次执行，无 crash；运行时间和次数不是覆盖率证明。
- cargo package 与全新 Rust 1.85 解包消费者：数字巨大指数、guarded drift、Unicode 和正反向通过，解包源码与正式测量一致。
- 独立 target 实际编译最终源码：380 fixtures × 8 模式 = 3,040 公开输出/错误记录，加 17 × 2 = 34 内部边界；完整 forward/inverse wire、错误阶段/path/message 与基线一致。三个新增永久测试通过。

详细命令、stdout/stderr、来源归因见 [checks.json](checks.json)，未将旧阶段通过冒充最终源码通过。

## Adopt / adapt / build 与剩余工作

Adapt [Go jsondiff v0.7.1 的局部实际成本处理](https://github.com/wI2L/jsondiff/blob/v0.7.1/differ.go#L190)，Build 当前 fine-first 候选的稳定槽位处理。
拒绝直接照搬递归 suffix 改写：会改变 Palim 候选顺序、copy 可用性与 guard 合同。
拒绝通用动态依赖树/Fenwick/source index：现有证据支持 Replace-only，混合来源读取与数组移位需要单独证明。
不新增 RawValue、全局数字缓存或自建 parser：小输入数据没有证明统一收益，数字精度合同仍必须保持。

Replace-only 的选择/index 工作随原始操作及其候选祖先总数增长，取决于 pointer 深度；实际候选 payload 序列化和完整验证仍有成本。
混合 Add/Remove/Move/Copy/Test 的通用 rationalizer 仍可能 O(P×N)，matcher/Histogram/fuzzy 的既有边界也未改变。
下一优先级是用真实混合操作 workloads 定位成本、证明依赖失效规则，再决定最小优化；小 JSON 全流程需要继续分离 serde_json 解析/序列化和库内工作。
这些尚未解决，不能据本轮结果声称任意输入线性、全部竞品更快或全面最优。

## 复现证据

[evidence.zip](evidence.zip) 含四次正式阶段、最终源快照/fixtures/probes/locks/runner 与二进制 hashes、独立 oracle、profile、pointer/core/layout 有界实验、实际检查日志、解包消费者和修改前设计决定。
ZIP 中 MANIFEST.json 对每个 payload 保存 SHA-256，打包时执行 CRC 与逐项 hash 校验。编译 target、.git、凭据不进入证据包。
runner 保存最初绝对路径用于归因；迁移机器复现时需将 baseline/candidate 与冻结输入路径映射到解包位置。
