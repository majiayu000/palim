# 新增 JSON 成员复制优化 — 2026-10-06

## 修改与原因

基线采样的新增对象路径出现 `Value::serialize → SerializeMap → NumberValueEmitter → Number::from_str`。本地 serde_json 1.0.151 源码确认 NumberValueEmitter 先执行 `value.to_owned()` 再 parse，因此在已拥有 Number 字符串的情况下多了一次临时复制。对象通用序列化还逐个插入 Map。采样包含启动阶段，不据其估算精确耗时占比。

本轮只修改直接生成路径的新增对象成员复制：克隆现有 Value/Map 结构，再在副本中递归规范化数字；仍使用与原实现相同的 Number::from_str 解析器，保留非规范数值的规范化结果及无效 unchecked Number 的失败行为。省去通用 Serde 包装及 Map 逐项插入。分配计数是整体结果，不能从单个局部改动推断总分配减少。未增加 API、依赖、配置、版本或生产 unsafe。

src/export.rs 增加复制函数及两项回归，benches/core.rs 增加 2,000/10,000 项新增对象 benchmark。原有其他会话修改保留。

## 方案选择

第一版递归 collect 重建 Map 在新增对象上更快，但短数组普通/native 生成稳定退化约 26%，宽对象请求分配字节增约 34%，因此弃用。禁用内联及 root-array 路由两个对照也没有消除退化。采用克隆结构后原地规范化的版本，定向复测消除了短数组退化，三个分配探针与基线完全一致。原生数组也受首版影响，说明不能归因于 RFC 导出开销；具体编译机制仍未证明。各方案及首版完整数据均保留。

## 正式测量

Core：已解析输入开始，包含输出销毁。Pipeline：同一紧凑输入字节的解析、生成、UTF-8 序列化及销毁。每组 3 个独立进程，每进程 9 批；表格为进程中位数的中位数。所有计时顺序执行，交替先后顺序，编译/测试/fuzz 在计时前结束。基线包含之前的本地优化，并非已发布 0.1.4。

| 场景 / 模式 | 基线 Core ms | 新版 Core ms | 速度比 | 基线 Pipeline ms | 新版 Pipeline ms |
|---|---:|---:|---:|---:|---:|
| add-wide-100 / plain | 0.027861 | 0.020466 | 1.36× | 0.063254 | 0.055300 |
| add-wide-2000 / plain | 0.672245 | 0.444314 | 1.51× | 1.504521 | 1.299250 |
| add-wide-10000 / plain | 3.988542 | 3.037729 | 1.31× | 8.837708 | 8.243625 |
| add-wide-100000 / plain | 45.642708 | 40.633833 | 1.12× | 100.117667 | 96.708583 |
| add-wide-10000 / optimized | 6.645250 | 5.214875 | 1.27× | 10.864584 | 10.508396 |
| add-wide-10000 / plain-guarded | 8.965250 | 8.382479 | 1.07× | 13.971417 | 13.725959 |
| file_migrations_move_copy_5000 / plain | 3.200209 | 2.954958 | 1.08× | 14.513334 | 14.321083 |
| file_migrations_move_copy_5000 / optimized | 37.921541 | 37.508833 | 1.01× | 46.000542 | 45.537084 |
| file_migrations_move_copy_5000 / guarded | 73.571875 | 71.759583 | 1.03× | 86.403375 | 84.726417 |
| scalar-guards-100000 / plain | 14.378459 | 14.299750 | 1.01× | 75.746625 | 75.156541 |
| scalar-guards-100000 / native | 48.827500 | 48.489041 | 1.01× | 105.215583 | 105.020000 |
| mixed-array-last-100000 / plain | 14.734500 | 14.579209 | 1.01× | 76.019500 | 76.048958 |
| rotate-2000 / plain | 0.225475 | 0.224632 | 1.00× | 0.377796 | 0.380507 |
| rotate-2000 / native | 0.202710 | 0.204665 | 0.99× | 0.359267 | 0.358635 |
| rotate-1000000 / plain | 193.555667 | 214.417000 | 0.90× | 261.161958 | 294.616667 |
| small-config-edit / plain | 0.002474 | 0.002493 | 0.99× | 0.009941 | 0.009888 |

`plain` 为普通操作；`optimized` 为 factorize/rationalize；`plain-guarded` 只加 test；`guarded` 三项均启用；`native` 为可逆 Delta。仅新增对象成员调用本轮函数，其他场景是退化控制。

## 百万数组追加复测

正式批次百万数组的进程间范围较宽，故额外运行五对交替进程；所有原始结果都保留。这里仅报告测量，不能证明所有数组输入无退化或已经获得稳定加速。

| 版本 | Core ms | Pipeline ms |
|---|---:|---:|
| baseline | 204.733083 | 255.928334 |
| candidate | 209.159708 | 268.344541 |

## 同次 Rust json-patch 4.2.0 比较

相同 serde_json 精度特性、输入 Values、计时边界及输出销毁。三个场景的紧凑 patch 字节相同。

| 场景 | Palim Core ms | json-patch Core ms | Palim Pipeline ms | json-patch Pipeline ms |
|---|---:|---:|---:|---:|
| add-wide-10000 | 3.037729 | 2.037984 | 8.243625 | 7.119438 |
| add-wide-100000 | 40.633833 | 23.113792 | 96.708583 | 78.117167 |
| file_migrations_move_copy_5000 | 2.954958 | 2.671885 | 14.321083 | 13.637375 |

## 分配计数

已解析输入上生成一次，独立插桩二进制。Calls 包含 allocation/reallocation，Peak 是额外请求的存活字节，不是 RSS。全部探针在销毁输出后恢复到原始存活字节。

| 场景 | 版本 | Calls | Requested bytes | Peak extra live bytes |
|---|---|---:|---:|---:|
| add-wide-10000 | baseline | 81,686 | 8,388,079 | 7,664,929 |
| add-wide-10000 | candidate | 81,686 | 8,388,079 | 7,664,929 |
| add-wide-100000 | baseline | 816,689 | 83,033,727 | 76,750,561 |
| add-wide-100000 | candidate | 816,689 | 83,033,727 | 76,750,561 |
| file_migrations_move_copy_5000 | baseline | 75,057 | 10,890,012 | 8,953,511 |
| file_migrations_move_copy_5000 | candidate | 75,057 | 10,890,012 | 8,953,511 |
| mixed-array-last-100000 | baseline | 304,096 | 20,827,409 | 11,131,643 |
| mixed-array-last-100000 | candidate | 304,096 | 20,827,409 | 11,131,643 |

## 已完成验证

- Debug / Release：188 / 188 项测试通过。
- `cargo fmt --check`、Clippy 全目标且 warnings 为错误、Rust 1.85 library check。
- Release fixture_runner 及 JS 互通：1073 条，无失败。
- Nightly fuzz：207601 次，61 秒，无失败（预算 60 秒）。
- 冻结的 3,305 条公开/私有差分记录，优化决策、成本与错误字节一致。
- 93 个 RFC 计时进程在计时外检查独立 json-patch 正向应用和 Palim 逆向恢复；12 个 Native 进程检查 Palim 正向应用和 reversed Delta 恢复。所有配置的前后输出 hash 相同。
- 新回归覆盖嵌套数组/对象、转义键、大整数、极大指数、负零/冗余指数等原始表示、源输入不变及无效 unchecked Number 的同样失败消息。

## 结论的限制

- 优化针对新增成员的大型容器。成员迁移仍包含深度检查、键扫描、路径及值复制等成本；依据实测表格判断，不能声称已领先 json-patch。
- 默认排序 Map、单台 Apple M2 Max、warm 顺序测量；没有 cold、并发、尾延迟、跨平台或 RSS 数据。微小差异及过程间重叠不作为稳定收益。
- native Delta、数组匹配、guard、factorize/rationalize 算法没有修改。本轮代码尚未提交或发布。

## 证据

[measurements.json](measurements.json) 含来源/输入指纹、原始批次、进程中位数、分配、检查和试验数据。[evidence.zip](evidence.zip) 含冻结源码、集成测试、探针、输入、oracle、采样、日志与重建脚本。
归档 24,772,930 字节，425 个 payload 文件；SHA-256 `49a8876b7168d080ae3eb30718a42fdc69330e250ebddad80b74da903a24c5fe`。已验证各 payload hash 与 ZIP CRC。

归档已解压到独立目录，执行 `python3 rebuild.py --replay-small`：全部 payload hash、两份探针重建、小输入正向/逆向恢复及原始输出 hash 均通过，exit 0。重建回执保存在 measurements.json。
