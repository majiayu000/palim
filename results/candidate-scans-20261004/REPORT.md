# 候选扫描与 guarded 回退：2026-10-04

任务于10月3日开始，验收于10月4日完成。基线为 `08f3735ad3d1fe31c0ebe3a6142002ab9251d958`；正式候选由源码/锁/实际binary hash绑定，不能把计时时HEAD当作候选身份。仅修改RFC生成内部实现、一个必要回归，以及0.1.1版本与测量文档；无新公开API、依赖、parser、配置、缓存层或生产unsafe。

## 实测结果

- 小配置guarded **8.401 → 7.560 μs，1.11×**，修复上一轮约3%回退所发现的无用目标比较。
- 400对象guarded **11.711 → 8.882 ms，1.32×**；800对象 **38.202 → 27.471 ms，1.39×**。仍有逐候选扫描/前缀重建/操作压缩成本，没有线性最坏复杂度证明。
- after-first-accept控制普通压缩 **0.360927 → 0.356280 ms**，guarded **0.671250 → 0.589898 ms**。K8s普通压缩首版多出的12次分配已消除。
- 38种输入/模式、456记录的完整输出均与基线逐字节相同，独立应用和inverse通过。分配次数/请求总量/峰值变化全部保留在JSON，不从CPU提速推断内存下降。
- 普通400/800对象最终约慢2.1%/2.4%，小native约慢2.9%；其它控制也有小幅正负差异。保留样本，不宣称所有场景更快。

## 最小实现与验证依据

1. 原初次guarded replay仍实际执行并传播相同Error。仅当原patch多于1操作且有非root父候选时比较其最终目标；其它情况不会复用此证明，候选仍实际验证。
2. root候选选择所有操作，精确成本直接为 `Test(left)+Replace(right)+3`（两array括号和逗号），前缀仍有正确bytes/count。实际candidate_applies保留，不以原补丁目标证明代替root应用。
3. 接受一次替换后，原来就必须执行的selection扫描仅在tests=true时同时判定读写/guard独立性。跨Copy中间读、ancestor snapshot和array shift只让shortcut失败，继续完整replay；原Move/ancestor boundary rejection不变。
4. 共享per-operation判定必须内联：release机器码首版有2处真实`bl`，加入唯一 `#[inline(always)]` 后2处均消失。删除此属性后，最终源码与已完成语义验证的f809源码逐字节相同。

采用已有proof和前缀表的局部改进；前轮已核查 [Go jsondiff v0.7.1 rationalize源码](https://github.com/wI2L/jsondiff/blob/v0.7.1/differ.go) 的精确局部字节成本原则，未改变Palim父容器guards合同。没有新增技术栈或抽象层。

## 用测量淘汰的方案

- 全模式提前做proof：预算拒绝控制普通模式约慢5.3%，K8s额外12次分配。改为只有guarded合并，普通恢复原lazy证明。
- guarded-only但共享helper不内联：预算控制恢复，普通400/800仍稳定慢7.4%/10.8%；反汇编确认循环内函数调用。最终内联后缩到约2%差异。
- 两个阶段的456原始记录、source/binary hashes和未采用source都保留；不把其较好的其它行冒充最终结果。

## 正式测量边界

macOS arm64、Rust1.97.0。输入已解析，generation+输出析构计时；parse、serialize和正确性检查在外。两版本使用独立源码快照与target。计时二进制为System；另一二进制独立统计成功alloc/realloc请求。3warmups、至少10ms校准cap2048、9batches、3进程/版本交错顺序，表为进程中位数的中位数。38×2版本×2测量类型×3进程=456。

| 输入 / 模式 | 08f3735 ms | 最终 ms | 倍率 | 相同输出 B |
|---|---:|---:|---:|---:|
| small-config-edit / native | 0.001949 | 0.002005 | 0.97× | 28 |
| small-config-edit / optimized | 0.003320 | 0.003312 | 1.00× | 53 |
| small-config-edit / guarded | 0.008401 | 0.007560 | 1.11× | 101 |
| rotate-2000 / native | 0.307444 | 0.308042 | 1.00× | 27 |
| rotate-2000 / guarded | 0.521029 | 0.511721 | 1.02× | 34130 |
| disjoint-2000 / optimized | 2.402240 | 2.381359 | 1.01× | 34038 |
| disjoint-2000 / guarded | 21.061458 | 21.593375 | 0.98× | 68072 |
| ambiguous-ids-2000 / optimized | 7.432333 | 7.387271 | 1.01× | 44296 |
| ambiguous-ids-2000 / guarded | 9.767459 | 9.475354 | 1.03× | 88590 |
| many-accepted-100 / optimized | 1.299177 | 1.298745 | 1.00× | 10028 |
| many-accepted-100 / guarded | 1.689495 | 1.425302 | 1.19× | 18822 |
| many-accepted-400 / optimized | 8.462041 | 8.636354 | 0.98× | 40328 |
| many-accepted-400 / guarded | 11.711042 | 8.882041 | 1.32× | 76722 |
| many-accepted-800 / optimized | 27.074834 | 27.735958 | 0.98× | 80728 |
| many-accepted-800 / guarded | 38.202084 | 27.470833 | 1.39× | 153922 |
| unicode-three-edits / native | 0.429964 | 0.429728 | 1.00× | 311 |
| unicode-disjoint-24000 / native | 2.378740 | 2.375021 | 1.00× | 1080039 |
| single-text-edit / native | 0.025983 | 0.026145 | 0.99× | 48 |
| kubernetes-deployment-controlled / native | 0.006021 | 0.006035 | 1.00× | 499 |
| kubernetes-deployment-controlled / optimized | 0.060127 | 0.060089 | 1.00× | 543 |
| kubernetes-deployment-controlled / guarded | 0.125912 | 0.124923 | 1.01× | 919 |
| kubernetes-deployment-sparse / native | 0.001655 | 0.001697 | 0.98× | 27 |
| kubernetes-deployment-sparse / optimized | 0.003064 | 0.003054 | 1.00× | 52 |
| kubernetes-deployment-sparse / guarded | 0.006951 | 0.006617 | 1.05× | 100 |
| npm-package-controlled / native | 0.009085 | 0.009049 | 1.00× | 414 |
| npm-package-controlled / optimized | 0.034707 | 0.034654 | 1.00× | 610 |
| npm-package-controlled / guarded | 0.079636 | 0.081063 | 0.98× | 2506 |
| npm-package-sparse / native | 0.004247 | 0.004285 | 0.99× | 29 |
| npm-package-sparse / optimized | 0.006707 | 0.006756 | 0.99× | 52 |
| npm-package-sparse / guarded | 0.013146 | 0.012262 | 1.07× | 100 |
| synthetic-after-first-accept-100 / optimized | 0.360927 | 0.356280 | 1.01× | 4825 |
| synthetic-after-first-accept-100 / guarded | 0.671250 | 0.589898 | 1.14× | 9343 |
| typescript-config-controlled / native | 0.002416 | 0.002392 | 1.01× | 156 |
| typescript-config-controlled / optimized | 0.014002 | 0.013938 | 1.00× | 327 |
| typescript-config-controlled / guarded | 0.031234 | 0.030909 | 1.01× | 909 |
| typescript-config-sparse / native | 0.001890 | 0.001906 | 0.99× | 61 |
| typescript-config-sparse / optimized | 0.003416 | 0.003424 | 1.00× | 83 |
| typescript-config-sparse / guarded | 0.006618 | 0.006249 | 1.06× | 161 |

`requested_bytes` 是累计新请求尺寸；`peak_additional_live_bytes` 是相对开始点同时存活请求字节增量，不含预持有输入，不是RSS或物理allocator容量。所有输出析构后live增量为0。原始samples、processmedians、allocations和完整hash见 [measurements.json](measurements.json)。共享机器短时结果不是P99或跨平台性能排名。

## 公开文档形状的验证

固定官方commit的 [Kubernetes nginx Deployment](https://raw.githubusercontent.com/kubernetes/website/77db41e9c776b614fdb31de4cc6c8e9a70673817/content/en/examples/controllers/nginx-deployment.yaml)、[jsondiffpatch npm manifest](https://raw.githubusercontent.com/benjamine/jsondiffpatch/a60db8a232f92ee4b987c14f43f77402885d1a5a/packages/jsondiffpatch/package.json)、[TypeScript配置](https://raw.githubusercontent.com/benjamine/jsondiffpatch/a60db8a232f92ee4b987c14f43f77402885d1a5a/tsconfig.json)。各有sparse和controlled edits，共6输入×3模式；另外101对象synthetic after-first-accept控制仅2压缩模式。

原文shape来自公开项目，右侧改动由本轮构造，不是实际生产事件/下游采用，也不声称K8s战略合并或TypeScript语义验证。斜杠key、深层对象、容器数组、新root key及父guard覆盖本轮风险。sources、fixturehash、转换记录与CC-BY-4.0/MIT许可证保存于归档。

## 解析差距：独立08f3735诊断

小配置双parse→diff→serialize约9.383 μs，单独parse/drop7.352 μs，约78%预算；phase medians不可精确相加。库接收Value，解析在caller。caller保留旧Value只parse新输入约5.657 μs，属于已有API的调用边界改变，不是“两输入都parse”排名。from_str约3.1%改善，输出buffer复用约0.6%没有明确稳定收益。

官方serde_json1.0.151源码和独立分配探针确认AP数字scanner暂存String，普通整数再构造owned Number。没有为了此小输入关闭精度或另造parser/API。该诊断不是最终新RFC候选的新JS排名；小JSON完整流程的既有JS差距仍未解决。

## 真正完成的检查与范围

- 最终139780dd源码：debug/release各167项、fmt、Clippy all-targets `-D warnings`、Rust1.85库检查通过。
- 同语义f809版本：2,984公共wire和22边界wire与08f逐字节相同，正向/逆向通过，49focused通过；最终版本仅多inline属性，机械diff已核。没有把旧binary验收伪称最终binary重跑。
- 最终139版本456条正式测量的正逆验证均通过；新增永久golden覆盖接受前一父后masked crossCopy、ancestor Add guard和array shift的fallback。
- 1,073个必需JS互通全通过；562个合法JS自身成功inverse默认fuzzy还原，4个格式错误仍Error，上游507自身失败另列。
- ASan fuzz max_len2048/RSS1024MiB/seed20261001，61秒322,383次，无失败；命令墙钟含构建104.56秒，次数不是行覆盖率。
- cargo package打包/解包通过；无预置lock的Rust1.85消费者使用解包palim0.1.1，guarded drift、精确数值与长Unicode正逆通过。package生产sourcehash与测量snapshot一致。
- 独立review无正确性阻塞；本报告是本地候选快照，远端三OS CI与正式发布随后完成，状态以 [Actions](https://github.com/majiayu000/palim/actions) 和 [v0.1.1 Release](https://github.com/majiayu000/palim/releases/tag/v0.1.1) 为准。

[checks.json](checks.json)保存实际退出码/完整输出/源码身份、清楚分开的f809语义证明、inline机器码和最终139验收，不把语料相加为独立覆盖率。

## 复现与剩余工作

[evidence.zip](evidence.zip)含三轮raw records/metadata，最终双版本snapshots/probes/locks，公开fixtures/原文/license、parser原始source/inputs、oracle、机器码核对和MANIFEST。旧阶段保留为淘汰证据。解包后修改private probe path依赖/driver fixture路径，分别使用独立target；orchestration中的runner用baseline/candidate/output三个路径，extra fixture目录指向usecases/fixtures。

```sh
python3 orchestration/palim-scans-benchmark.py /path/to/baseline /path/to/candidate /path/to/new-output
cargo test --locked
cargo test --release --locked
cargo clippy --all-targets --locked -- -D warnings
cargo +1.85.0 check --lib --locked
cargo build --release --examples --locked
npm ci --prefix tools --ignore-scripts --no-audit --no-fund
node tools/interop.mjs
cargo +nightly fuzz run core -- -max_total_time=60 -max_len=2048 -rss_limit_mb=1024 -seed=20261001
cargo package --allow-dirty --locked
```

重复selection、guard-prefix重建和Vec compaction仍有二次成本；普通大输入本次约2%小幅回退。全局最小补丁、任意输入最快、所有功能领先和生产采用都没有证明。后续复杂索引/borrowed parser需要实际独立收益证据再设计。
