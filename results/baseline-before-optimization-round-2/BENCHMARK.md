# Benchmark 与验证

本轮任务日期：2026-10-01。测量时间戳按运行机器 UTC 原样保存。Apple M2 Max，96 GiB，Rust 1.97.0，Node 24.14.0。串行执行，先验证输出正确，再记录时间。

## 本轮实际完成

- Debug 与 release：98 个 integration tests、1 个 unit test、1 个 doctest，全部通过。
- 固定种子性质测试：25,344 个生成案例；另有唯一元素长度 2–7 的 5,906 个非恒等排列、373 组冻结文档和 92 个公共 RFC 6902 用例。语料存在重叠，不将计数相加称独立覆盖率。
- JS 互通：1,073 组 Rust delta 正向、Rust inverse 反向、Rust RFC 正向、JS delta 到 Rust 正逆还原全部通过。
- 原版 JS 在同一冻结语料中自己的 inverse 有 507 次失败，单独记录；这不是业务失败率，也不代表 Rust 能应用所有 JS inverse。
- fmt、all-targets Clippy（-D warnings）、Rust 1.85 library check 均通过。
- Criterion：62 项；原生横向：36 个测量、4 个预算跳过；标准横向：54 个独立进程测量，全部正确。

最终检查、源码/测试/基准哈希和 package 状态见 [verification.json](results/verification.json)。

## 原生 delta：Rust 与 JS

双方 parse 两份 JSON → diff → serialize 完整可逆 delta。相同 object_hash 和 UTF-16 文本阈值。每个进程 20 次预热、5 个计时批次，表中为进程内批次中位数；不是端到端服务 P99。纯 diff、patch、reverse 另行保存。

| 场景 | Rust pipeline ms | JS pipeline ms | Rust / JS delta 字节 |
|---|---:|---:|---:|
| small-config-edit | 0.01484 | 0.00674 | 28 / 28 |
| rotate-2000 | 0.46187 | 44.67854 | 27 / 27 |
| keyed-rotate-edit-2000 | 2.56925 | 79.02800 | 74 / 74 |
| crates-metadata-rotate-edit-100 | 0.80870 | 1.00440 | 141 / 141 |
| long-text-ascii | 0.45995 | 0.30487 | 56 / 83 |
| long-text-unicode | 0.62970 | 0.30806 | 56 / 151 |

16 对可比较输入中，Rust 快 9 对。单项移动约快 96.7 倍、ID 移动加修改约快 30.8 倍；小配置和长文本仍较慢。

长 ASCII / Unicode pipeline 从实施前约 1.041 / 0.955 ms 降到 0.460 / 0.630 ms。变化同时包含字符串前后缀裁剪、数字表示与其它代码变化，不能据此分离各因素的收益。纯 diff 仍比 JS 慢，分别约 21.3 / 17.0 倍；解析序列化会掩盖这部分差距。

20k 原版 LCS 的 rotate/reverse/disjoint/sparse 四项按事先资源预算跳过，未实测 OOM/超时；不是失败记录。原始结果见 [comparison.json](results/comparison.json)，输入哈希与环境见 [benchmark-environment.json](results/benchmark-environment.json)。

## 标准 JSON Patch：同一完整边界

Typed json_patch::diff、本库 plain、本库 optimized 均 parse 同一对冻结 JSON bytes → 生成 typed RFC Patch → serialize。plain 关闭 factorize/rationalize/tests，optimized 开前两者、不开 tests。没有给 json-patch 额外 Value 转换成本。公开 API 的基线验证和投影也计入本库时间。

每种输入/引擎独立启动 3 次进程，轮换执行顺序。每进程 20 次预热，按 pipeline 至少 25 ms 校准（最多 16,384 次），取 10 个原始批次。下表时间是三个进程内中位数的中位数。所有实际输出由独立标准应用入口检查正确后才计时。

| 输入 | json-patch ms / 字节 / 条数 | 本库 plain ms / 字节 / 条数 | 本库 optimized ms / 字节 / 条数 |
|---|---|---|---|
| small-config-edit | 0.01066 / 53 / 1 | 0.02022 / 53 / 1 | 0.04019 / 53 / 1 |
| rotate-2000 | 0.49296 / 112,891 / 2,000 | 0.62213 / 42 / 1 | 0.74807 / 42 / 1 |
| disjoint-2000 | 0.47737 / 112,891 / 2,000 | 5.71978 / 165,781 / 4,000 | 14.07843 / 34,038 / 1 |
| keyed-rotate-edit-2000 | 3.58992 / 479,232 / 8,000 | 4.17544 / 116 / 2 | 10.87917 / 116 / 2 |
| long-text-ascii | 0.25488 / 180,044 / 1 | 1.17298 / 180,044 / 1 | 1.43033 / 180,044 / 1 |
| halfswap-500 | 0.11976 / 27,891 / 500 | 0.64996 / 10,001 / 250 | 0.73088 / 8,538 / 1 |

移动与 ID 负载输出小得多，但生成时间仍慢于位置式 json-patch。全异数组 optimized 改用根 replace，体积小于 plain，也显著增加 CPU。不能宣称标准输出整体最快。

halfswap-500 采用 500 个字符串：plain 为 250 moves、10,001 B，optimized 按字节选择 1 replace、8,538 B；替换根不代表保留了移动语义。实施前 9,075 moves 的例子是 500 个整数，与本表字节数不可混比。相同整数纯排列的新验收为 250 moves。

原始 54 条记录、每进程 10 个 samples、fixture/source/binary hashes、RSS 与环境保存在 [standard-benchmark-20260930T174815.824072Z](results/standard-benchmark-20260930T174815.824072Z/comparison.json)。

## Criterion：库内部成本

20 samples、100 ms warmup、300 ms target、5,000 resamples。超出目标时 Criterion 自动延长采集。下表为 slope estimate；flat sampling 使用 mean。完整区间与 samples 见 [criterion.json](results/criterion.json)，日志见 [criterion-run.log](results/criterion-run.log)。这不是竞品统一排名。

| 测量 | ms |
|---|---:|
| core/diff/identical-20000 | 0.162695 |
| core/diff/rotate-20000 | 3.291492 |
| core/diff/keyed-20000 | 6.849011 |
| core/diff/text | 0.186111 |
| standard/export-precomputed/halfswap-500 | 0.344592 |
| standard/optimized/disjoint-2000 | 14.067998 |
| compare/unordered-multiset-2000 | 0.982921 |
| compare/absolute-tolerance-2000 | 0.766179 |
| merge/diff-settings-1000 | 0.185319 |
| merge/apply-settings-1000 | 0.156274 |
| core/patch/keyed-2000 | 0.374060 |
| owned/patch-preowned/keyed-2000 | 0.026475 |

owned 的 baseline clone 在计时外准备；它衡量已有所有权的应用成本，不可把相对 borrowed 的差距称端到端吞吐提升。数字启用任意精度后，部分数字数组 diff/patch 比实施前基线慢；新增能力不是无成本。

基准发现并修复了 copy 优化反复扫描不可能匹配标量的问题。同一个整数 disjoint-2000 Criterion 从约 236.6 ms 降到 14.3 ms（定向复测），最终完整轮为约 14.1 ms；未达到 plain 的约 5.5 ms。优化前 estimates 保留在 [criterion-before-copy-cache.json](results/criterion-before-copy-cache.json)。

## 历史与复现

实施前数据和源码校验记录保留在 [baseline-20260930](results/baseline-20260930/verification.json)。旧原型和旧只读探针见 [COMPARISON.md](COMPARISON.md) 的历史部分，不用旧数值代表当前源码。

```sh
cargo test --locked
cargo test --release --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
cargo +1.85.0 check --lib --locked
cargo bench --bench core --locked -- --noplot
npm ci --prefix tools --ignore-scripts --no-audit --no-fund
cargo build --release --example fixture_runner --example standard_bench --locked
node tools/interop.mjs
python3 tools/benchmark.py
python3 tools/standard-benchmark.py
cargo package --allow-dirty --locked
```

单机、小样本、冻结输入；无业务加权和跨平台数据。RSS 是整个进程峰值，包含解析、验证、预热与运行时，不是算法分配量。没有测 Go/Java/C++ 的统一运行环境，因此不输出全生态速度排名。
