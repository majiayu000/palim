# Guard 成本复用与对象查找：2026-10-03

## 结果与范围

相对 `13ebc709cd38a09799d73ec9d80d8d05b73b0581`，本轮仅改两个生产模块和一个已有 benchmark runner，增加3个回归测试、报告与原始证据。公开 API、依赖、选项、数值/错误合同、父候选顺序没有变化。计时时 HEAD 仍是基线；候选身份由完整源码和实际二进制 SHA256 绑定。

**实测：** 400个独立对象的 guarded 生成 **187.458 → 11.890 ms，15.77×**；分配/重分配次数 **4,410,888 → 101,197**，累计请求字节 **212,697,524 → 10,453,675 B（约下降95.1%）**。峰值额外同时存活请求字节仍为 **1,891,121 B**，所以不能把累计字节降幅描述为峰值内存降幅。100对象 guarded 约快7.46×。

**实测：** 小配置原生已解析 diff **2.663 → 1.975 μs，1.35×**，分配量不变。该负载完整 parse→diff→serialize **Palim 9.615 μs、JS default 6.721 μs**，仍有差距。小 guarded 由8.305变8.553 μs（约慢3%）；旋转/全异/重复ID guarded 控制场景约慢1.6%–1.9%。保留回退样本，不宣称所有场景更快。

16种内部输入/模式完整输出与基线字节相同，独立正向/逆向应用成功。本轮只改生成性能，没有重新发布crate，也没有全面功能领先的证明。

## 实现与选择理由

### Guarded 候选的精确成本复用

原实现对每个候选重放整份文档并统计生成 guards。基线 profile（每规模一次因果诊断）中，800对象总795ms，完整候选模拟约640ms；不能用这组三次诊断冒充正式速度对照。

1. 原来已发生的第一次 guarded replay 现在只留下“数学上到达目标”的布尔值，scratch文档立即丢弃；错误仍由原 helper 传播。
2. 仅当已有独立替换证明成立、原补丁确实达到目标、且外部 Add/Copy/Move 的目标父容器不是该子树祖先时，复用已有 `GuardCost` 的精确组成本。后一个条件防止外部 guard 读取子树中间状态。
3. 每组是自动 tests + 原操作。减掉旧前置逗号，删除选中组，以精确 `Test(old)+Replace(new)` 替换第一组，然后按新前缀补逗号和操作数。非连续选择、重复写、Unicode/转义和后续父替换都使用真实旧成本。
4. 跨边界Copy/Move、祖先读取、数组移位及未证明情况继续完整模拟。根/单操作候选仍实际执行；最终 guards 仍真实生成。严格“更小才接受”、目标/深度与原错误处理保持原合同。

**源码已核查：** [Go jsondiff v0.7.1 的 rationalize](https://github.com/wI2L/jsondiff/blob/v0.7.1/differ.go) 使用准确局部序列化长度比较。决定适配这个原则到已有前缀表；没有采用其 guard 合同，因为 Palim 的 Add/Copy/Move 可能认证整个父容器。拒绝 shadow缓存、读写集合层、新backend/配置和重排候选。风险是错误复用外部认证读；以上保守条件、对照完整重放与边界oracle作为验收。

仍要逐父扫描操作并重建前缀；最坏情况仍有 O(父候选数×操作数) 标量成本。本轮减少重复 DOM 克隆、应用和序列化，**没有证明线性最坏复杂度**。

### 相同字段对象跳过第二轮查找

第一轮在原属性过滤之后统计 `Some(new)` 次数，且在未修改 primitive 的 continue 之前递增。若已找到的键数等于右对象键数，右对象没有新增键，可跳过仅处理新增键的第二轮查找。拒绝的属性只会漏掉优化，继续原循环；回调顺序、删除/新增及Value/wire表示不变。没有新哈希表、分配或公开配置。

## 最终内部测量

macOS arm64，Rust1.97.0；基线/候选冻结源码、独立target、不同真实二进制SHA。生成和输出析构计时，输入已解析；解析、序列化和正确性检查均在计时外。独立json-patch应用和原生逆向先通过。

每输入/模式3次预热，至少10ms校准（上限2048次），9批次，3独立进程/版本，轮换顺序。时间取进程中位数的中位数。16×2版本×2种测量×3进程=192记录。计时为默认System；分配计数使用另一二进制。原始批次和hash见 [measurements.json](measurements.json)。这是短时单机结果，不等于服务P99。

| 场景 / 模式 | 13ebc70 ms | 本轮 ms | 倍率 | 分配/重分配次数 | 累计请求 B |
|---|---:|---:|---:|---:|---:|
| small-config-edit / native | 0.002663 | 0.001975 | 1.35× | 20 → 20 | 1482 → 1482 |
| small-config-edit / optimized | 0.003992 | 0.003365 | 1.19× | 48 → 48 | 2438 → 2438 |
| small-config-edit / guarded | 0.008305 | 0.008553 | 0.97× | 213 → 213 | 11725 → 11725 |
| rotate-2000 / native | 0.311544 | 0.309820 | 1.01× | 8044 → 8044 | 452825 → 452825 |
| rotate-2000 / guarded | 0.519383 | 0.527518 | 0.98× | 14105 → 14105 | 848330 → 848330 |
| disjoint-2000 / optimized | 2.424281 | 2.430859 | 1.00× | 40409 → 40409 | 2965290 → 2965290 |
| disjoint-2000 / guarded | 21.059709 | 21.405000 | 0.98× | 103741 → 103742 | 5653999 → 5654015 |
| ambiguous-ids-2000 / optimized | 7.435625 | 7.414959 | 1.00× | 155744 → 155744 | 9655133 → 9655133 |
| ambiguous-ids-2000 / guarded | 9.314896 | 9.487229 | 0.98× | 211888 → 211899 | 15198262 → 15263750 |
| many-accepted-100 / optimized | 1.320922 | 1.334391 | 0.99× | 19689 → 19689 | 1001554 → 1001554 |
| many-accepted-100 / guarded | 12.914458 | 1.731703 | 7.46× | 306244 → 25401 | 14438602 → 1647165 |
| many-accepted-400 / optimized | 8.429375 | 8.569104 | 0.98× | 78385 → 78385 | 4029464 → 4029464 |
| many-accepted-400 / guarded | 187.458208 | 11.889542 | 15.77× | 4410888 → 101197 | 212697524 → 10453675 |
| unicode-three-edits / native | 0.442414 | 0.446366 | 0.99× | 127 → 127 | 3343226 → 3343226 |
| unicode-disjoint-24000 / native | 2.493209 | 2.408734 | 1.04× | 100 → 100 | 8234061 → 8234061 |
| single-text-edit / native | 0.026313 | 0.026455 | 0.99× | 23 → 23 | 801 → 801 |

`requested_bytes` 为成功alloc/realloc的新请求尺寸累加；`peak_additional_live_bytes` 为开始点之后同时存活的请求字节增量，不含预先持有的输入，不是RSS或分配器物理容量。每次输出析构后live增量均0。

前三种小配置场景和100/400对象 guarded以外，多数差异在约±3.5%，没有归因为具体源码行或当成新收益。guarded 全异额外1次/16 B、重复ID额外11次/65,488 B：从源码推断与新增的第一次数学目标相等检查有关，尚未逐分配追踪证实；峰值保持相同。未增加生产unsafe；探针GlobalAlloc中的必需unsafe只透明转发System，计时二进制不加载计数器。

## 本轮新鲜竞品对照

固定JS jsondiffpatch0.7.6、Rust json-patch4.2.0、Go jsondiffv0.7.1，最终源码用全新独立target构建。4个RFC输入×6引擎×3进程=72、4个native输入×3引擎×3进程=36，共108个串行完成进程。RFC中36个Go结果经独立JS verifier实际验证；另36个Palim/json-patch记录在runner计时外用json-patch实际应用检查。没有把108都说成独立JS验证。

### Native：parse两份JSON→diff→serialize可逆delta

双方设置同一ID callback；JS default为position未指定，no-position显式关闭。各输入正向与inverse都成功；完整delta字节数如下。程序包含各自正常运行时边界，未统一跨语言分配器。纯diff小配置 Palim2.008 μs、JS default5.288 μs；加入解析/序列化后顺序相反。

| 场景 | Palim ms | JS default ms | JS no-position ms | 三者delta B |
|---|---:|---:|---:|---|
| small-config-edit | 0.009615 | 0.006721 | 0.006717 | 28 / 28 / 28 |
| rotate-2000 | 0.471221 | 38.680333 | 39.208208 | 27 / 27 / 27 |
| disjoint-2000 | 1.461577 | 286.685166 | 290.888041 | 111790 / 111790 / 111790 |
| unicode-three-edits | 0.772167 | 1.220628 | 1.182507 | 311 / 311 / 311 |

### RFC：parse两份JSON→typed JSON Patch→serialize

plain关factorize/rationalize/tests，optimized开前两项。Go optimized同样开启factorize/rationalize，数组身份/位置匹配合同不完全相同。每格 ms / B / 操作数；另外两个Go模式完整保存在JSON。

| 场景 | json-patch | Palim plain | Palim optimized | Go optimized |
|---|---|---|---|---|
| small-config-edit | 0.010389 / 53 / 1 | 0.009726 / 53 / 1 | 0.010866 / 53 / 1 | 0.026460 / 53 / 1 |
| disjoint-2000 | 0.472815 / 112891 / 2000 | 0.484213 / 112891 / 2000 | 2.620423 / 34038 / 1 | 14.665083 / 34038 / 1 |
| long-text-ascii | 0.248632 / 180044 / 1 | 0.258663 / 180044 / 1 | 0.322129 / 180044 / 1 | 2.710133 / 180044 / 1 |
| unicode-three-edits | 0.444313 / 230060 / 1 | 0.453241 / 230060 / 1 | 0.541038 / 230060 / 1 | 4.634471 / 230060 / 1 |

全异数组 optimized和Go输出相同34,038 B根替换，位置式json-patch为2,000条/112,891 B，更快但更大。RFC文本替换整串，与native311 B文本patch不是同一功能。没有全部库/全部输入/功能的排名。

## 实际完成的验证

- 最终debug/release各166项（含3个新增回归）；fmt、Clippy all-targets `-D warnings`、Rust1.85库检查通过。
- 公共373对×8选项=2,984：冻结源码独立target重编译的wire与13ebc70逐字节相同，正向/逆向成功。
- 8组边界×guarded/普通=16：外部祖先snapshot、跨Copy中间读被后写掩盖、重复写和数组移位，逐字节相同、正向/逆向成功。新增测试逐组比较复用成本与完整重放，拒绝“原补丁没达到目标”shortcut，保留真实初始Error。
- 原生373冻结对及4输入×4callback模式完整delta/inverse/回调/RFC stdout与基线一致。各类语料可能重叠，不相加称独立覆盖率。
- JS互通1,073个必需路径全通过；上游自身inverse507失败单列。自身成功566中合法562均默认fuzzy还原，4个格式错误仍返回Error。
- ASan结构化fuzz：seed20261001、max_len2048、RSS上限1024MiB；61秒300,518次执行，无失败。命令墙钟包含构建共104.93秒；次数不是行覆盖率。
- `cargo package`打包并解包验证通过；Rust1.85从无预置lock的消费者使用解包源码，guarded drift、精确数值和长Unicode正逆通过，最终源码hash与测量快照一致。
- 独立只读生产diff审查无阻塞。未重跑远端CI/跨平台运行；未推送或发布。

[checks.json](checks.json)含真实完整stdout/stderr、退出码、oracle与源码hash。互通第一次因它依赖的release示例尚未构建报ENOENT，构建结束后重跑成功；两次记录均保留。两名agent最初同名package共享target的trace/private oracle因缓存未重编译而被废弃，最终证明只使用独立target且真实binarySHA不同的重建结果。

## 复现与剩余差距

```sh
python3 tools/remaining-hotspots.py /path/to/13ebc70 /path/to/candidate /path/to/new-output
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

[evidence.zip](evidence.zip)含最终快照、双版本probes/locks、fixtures、原始samples/metadata、pinned竞品源码runner/npm-Go锁、oracle、审查和MANIFEST。解包后将probe Cargo.toml内path依赖及私有driver fixture绝对路径指向保存的sources/fixtures，分别用独立target构建；competitor runner里的C/B/P/O路径改成对应解包位置。公开内部对照runner接受路径参数。二进制不打包，真实SHA和构建日志保存。

- 尚有逐候选标量扫描/前缀重建的二次成本，且保守回退仍需完整重放。
- 小JSON完整流程仍慢于JS；未有保持精确数值合同的替换parser实证。
- 小guarded当前约3%回退。没有任意输入最快、全功能或全球最小补丁证明。
