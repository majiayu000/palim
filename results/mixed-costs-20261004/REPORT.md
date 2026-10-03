# Palim 0.1.3：混合操作的重复成本

日期：2026-10-04。基线为已发布 0.1.2 `4b8e9b3`。
最终 RFC 源码 `1d20138b…`；其它九个 Rust 源文件未改。
本页记录实际提交前本地验收；后续提交绑定的 CI、registry checksum、下载包源码与消费者验证见 [v0.1.3](https://github.com/majiayu000/palim/releases/tag/v0.1.3)。

## 证据与实现

只读隔离插桩对四种合成文档形状各 100/400/800 × 两模式，共 24 次公开 API 调用；每次正反向应用通过。
800 个混合 Add/Remove/Replace 对象仅需要一次初始 guard replay，但 selection/prefix/compaction 仍分别遍历 1,598,800 / 1,602,000 / 1,602,000 槽。
末尾新增根字段的 guarded 案例触发 802 次完整 guard 模拟、1,602,803 个实际步骤。
800 次文件迁移有 800 个实际接受的 Move 候选；复制 960,400 个操作槽，其中 320,400 个 payload 的键/字符串 len 总计 280,850,400 B。
后一个数字是 owned UTF-8 长度诊断，**不是 allocator 请求或 RSS**。插桩时间不当作正式排名。

本轮只修改现有 RFC 优化器：

1. 现有稳定槽位路径扩到 Add/Remove/Replace。Add/Remove 的父在 left 必须是 Object；无 Move/Copy/Test。fine-first、原始选择索引、严格祖先写入标记、真实初始 guard、延迟原补丁验证、根实际验证全部保留。
2. guarded preflight 在任何变更前按原始 selected 首/末槽及 Add-parent 的原始有序索引判断快照。严格交错的外部祖先快照全回原 generic 路径。非交错快照还要求已取得的最终 shadow 与 right 使用严格 Value equality，防止 `1.0` 与 `1` 数学相等但序列化成本不同。
3. factorize 的 Move 候选用借用 dense iterator 执行同样的默认深度/数学比较验证；失败不修改补丁，成功后消费旧操作并放入 owned Move，省掉临时补丁和未改 payload 的复制。初始 remove-shadow 的真实错误、候选次序与收益门限不动。

没有新 API、依赖、feature、unsafe、parser 或动态依赖树。

## 正确性理由与具体边界

对象父后来变数组或被删掉，必有该父/其祖先的原始写入；冻结 strict-ancestor 标记拒绝更深候选。等于或高于该写入的候选吸收整个内部修改，不会漏外部移位。
fine-first 接受 A 后，未来候选只能包含 A 或与之不相交；原始选择集合过滤零权重后仍为正确操作顺序。
外部 Add 在边界 first 之前观察 left，在 last 之后观察精确 final target；严格交错读会观察不同中间值，必须完整重算。
先前压缩保留原 first，原 last 即使失活也只把之后候选的拒绝区间保守放宽。精确值 proof 只约束这个 shortcut，generic 的数学相等合同不变。
Move 借用验证将过滤后的操作重新 enumerate，dense 序号与旧 collect Patch 相同；失败只返回原有候选 boolean，初始错误仍原序号/path/message。

新增两个永久测试在原 0.1.2 和最终候选都实际编译通过：before/interleaved/after 根快照、`1.0`→`1` 一字节收益边界（7366→7365 B）、Move remove-after-add 接受、数组移位拒绝和初始错误优先次序。

## 正式测量

70 组输入/模式 × baseline/candidate × System timing/独立 allocator × 三进程 = 840 条记录。
3 warmup、至少 10ms 校准（最多 2048 次）、每进程 9 批次、版本交错；各进程中位数再取中位数。
两版本独立源快照、独立 target、fixture/runner/lock/binary SHA 全保存；CPU 窗口内无其它 agent 编译或测量。
每条记录完整 wire hash 相同，计时外独立 json-patch 正向/逆向应用通过。
普通模式是已解析 Value 的生成/销毁；pipeline-native 包括两个原始输入解析、diff、serialize 和全部临时值/输出销毁。

| 输入 | 模式 | 0.1.2 ms | 0.1.3 ms | 速度比 |
|---|---|---:|---:|---:|
| independent_add_remove_replace_800 | optimized | 40.213708 | 10.679167 | 3.766× |
| independent_add_remove_replace_800 | guarded | 47.746041 | 13.729125 | 3.478× |
| object_remove_replace_no_add_800 | optimized | 28.591125 | 7.284750 | 3.925× |
| object_remove_replace_no_add_800 | guarded | 28.739292 | 9.685375 | 2.967× |
| root_add_guard_overlap_800 | optimized | 40.419708 | 10.651166 | 3.795× |
| root_add_guard_overlap_800 | guarded | 1550.318541 | 14.721041 | 105.313× |
| file_migrations_move_copy_800 | optimized | 1817.791167 | 1698.115583 | 1.070× |
| file_migrations_move_copy_800 | guarded | 2060.940417 | 1935.964125 | 1.065× |
| small-config-edit | native | 0.001803 | 0.001847 | 0.976× |
| small-config-raw-pipeline | pipeline-native | 0.008940 | 0.009019 | 0.991× |
| unicode-disjoint-24000 | native | 2.365104 | 2.382276 | 0.993× |

所有 70 行、逐次 samples、分配请求和 wire hashes 见 [measurements.json](measurements.json)。
四种新增形状是配置/包文件清单风格的**合成可扩展用例**，不是生产事件；约 105× 只代表这组末尾根 Add + guarded 输入。
旧 Kubernetes/npm/TypeScript 控制为冻结公开文档的受控编辑，也不代表实际用户采用。

800 普通混合增删改峰值额外请求 live 3,112,071→2,697,833 B（约降 13.3%），guarded 4,387,199 B 持平。
800 文件迁移普通累计请求 4,594,113,446→3,886,769,369 B（约降 15.4%），峰值 10,901,450→9,507,044 B（约降 12.8%）；guarded 也有约 15.4% 累计请求下降。
根新增 guarded 的累计请求 1,091,107,020→14,636,266 B。计数为相对预存输入的请求 live，**不是 RSS/物理堆**；所有 allocator final_live_delta=0。

## 小输入候选被拒绝

另一个唯一候选尝试在生成 pointer 前跳过所有未变 String/Array/Object。npm/TS 完整流程分别快约 10.3%/7.5%，分配显著减少；但变更容器会额外做 Value equality。
原整数完整流程慢 3.1%，深层变更容器慢 9.5%，长字符串 core 慢 15.2%。60 个交错进程证据保留，**没有合入这个候选**。
基线小整数完整流程约 9.21 μs，parse 约 7.26、core 约 1.83 μs。没有改变 serde_json 精度 flags、错误或表示合同来换取排名。
最终 RFC 改动的完整小整数 pipeline 约慢 0.9%，小 native 约慢 2.4%；分配和相关 core 源码不变，具体编译布局/硬件因果未证明。
最终矩阵没有行低于 0.95×，这也不能证明所有可能输入没有回退。未宣称全面最快。

## 实际完成的检查

- fmt/diff whitespace、debug/release 各 172 项、Clippy all-targets `-D warnings`、Rust 1.85、release examples。
- 1,073 JS 必需互通通过；562 个合法上游自逆由默认 fuzzy 还原，4 个畸形头部仍报错，507 个上游自身失败单列。
- ASan fuzz 61 秒 / 413,355 次执行无 crash；次数不是覆盖率。
- cargo package 与新 Rust 1.85 解包消费者通过，十个 Rust 源文件与正式 benchmark 一致。
- 独立 final target：3,040 公开 +34 内部 +32 本轮定向输出/错误和基线一致，两永久测试通过。完整旧基线本轮复用此前真实编译的 0.1.2 相同源/lock；32 组与两个 baseline goldens 本轮 fresh 编译。具体归因在 checks.json 和 oracle bundle 中。

## 选择与剩余成本

Adapt [Go jsondiff 的成本优先方法](https://github.com/wI2L/jsondiff/blob/v0.7.1/differ.go)，Build 已有 stable slots 的对象操作和有界 chronology preflight；[evanphx/json-patch](https://github.com/evanphx/json-patch) 只作真实 apply/guard 合同比较，不替换 Rust backend。
拒绝通用 source/index/dependency 图，拒绝更换候选次序或 guard 语义，也拒绝把数学相等当序列化成本相等。

仍有成本：Move factorize 每轮 remove-shadow 和候选真实 replay 仍可二次；迁移大输入本轮只快约 7%。Move 缺失 destination 的父快照也可能产生二次序列化成本。
交错祖先快照、数组结构移位和 Copy/Move 来源依赖仍走通用 rationalizer，不能把当前证明推广为全局线性。
小 JSON 外部解析/序列化成本仍在；这个边界的优化应继续要求精度/输出/错误等价的统一收益。

## 证据包

[evidence.zip](evidence.zip) 含正式 sources/probes/fixtures/locks/原始记录/runner与二进制 hashes、24 单调用插桩、最终独立 oracle、拒绝的小输入候选、所有最终 gate 日志、解包消费者及任务开始 snapshot。
MANIFEST.json逐 payload SHA256 和 ZIP CRC 均实际校验。编译 targets、.git、凭据不进入包。原绝对路径用于归因，跨机器复现需映射到解包位置。
