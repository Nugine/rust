# 05 · 详细任务分解

目标：把 04 的五个阶段拆成**可直接认领、带验收标准**的具体任务。每个任务给出：做什么、涉及文件、验收标准、依赖。你可以照这个清单排期或建 issue。

标注约定：`[S]` 小、`[M]` 中、`[L]` 大（相对工作量，非工时承诺）。坐标见 roadmap/03。

---

## 阶段一 · 复现与建档

### T1.1 建立本地实验环境 `[S]`
- **做什么**：按教程 03 构建 stage1；准备 `/tmp/vfe/` 实验目录；把教程 10.1 的「产 `.ll` + grep 元数据」封成一个小脚本。
- **验收**：能对任意 `.rs` 一键得到 vtable 的 `!type` / `!vcall_visibility` / 调用点形态。
- **依赖**：无。

### T1.2 跑通现有三测试 `[S]`
- **做什么**：`./x.py test` 跑 `tests/codegen-llvm/virtual-function-elimination.rs`、`...-32bit.rs`、`tests/ui/codegen/virtual-function-elimination.rs`。
- **验收**：三者本地绿；能读懂每条 CHECK 断言的含义（教程 09.3）。
- **依赖**：T1.1。

### T1.3 六 case 对照表 `[M]`
- **做什么**：为教程 10.7 的六个场景各写最小 `.rs`，记录三项观察，形成表格。
- **验收**：表格完成，明确回答「私有 trait 单/多 CGU 各标几」「北极星例现状是否被过度优化」。
- **依赖**：T1.1。

### T1.4 确认 WPD 是否真触发 `[M]`
- **做什么**：用 run-make 或 `-Csave-temps` + `llvm-dis`（教程 10.6）检查 fat LTO 后，一个「唯一 impl」的虚调用是否真被去虚化。
- **验收**：给出明确结论（是/否/条件性），并记录观察方法，供阶段四复用。
- **依赖**：T1.1。**这条结论直接决定阶段四工作量，优先做。**

---

## 阶段二 · 保守化 + 回归测试

### T2.1 北极星回归测试（run-make）`[M]`
- **做什么**：把 01.5 的例子做成跨 crate run-make：上游 crate 含私有 `Foo` + `pub struct FooBox` + `pub fn make_foo` + `#[inline] pub fn f`；下游 crate 调用 `f(make_foo())`；fat LTO 链接并**运行**，断言行为正确（若被过度删除会崩/错）。
- **涉及**：`tests/run-make/`（新目录 + `rmake.rs`）。
- **进度**：已落地初版 `tests/run-make/virtual-function-elimination-cross-crate/`（私有 `Foo` + `pub struct FooBox` + `pub fn make_foo` + `#[inline] pub fn f`，下游 crate fat LTO 链接并运行，断言返回 `42`）。当前作为**回归守护**：正确实现下应绿；若 VFE 过度删除 `Foo::foo` 则崩溃/结果错。本地无法构建 rustc（CI LLVM 主机被 DNS 屏蔽），需由 CI 实跑确认当前是否已复现 miscompile。
- **验收**：在**当前实现**下能复现问题（或至少锁定危险的 vcall_visibility 数值）；修复后转绿。
- **依赖**：T1.4（了解 WPD 是否真触发，决定测试断言强度）。

### T2.2 更多逃逸场景测试 `[M]`
- **做什么**：补充 (a) public 泛型函数中调用私有 trait object；(b) blanket impl；(c) 多 CGU 下 restricted trait。参考 07 风险清单逐条建测试。
- **涉及**：`tests/run-make/` 和/或 `tests/codegen-llvm/`。
- **验收**：每个危险场景都有守护测试。
- **依赖**：T2.1（复用 run-make 脚手架）。

### T2.3 保守化 `apply_vcall_visibility_metadata` `[M]`
- **做什么**：在 `metadata.rs:1715/1724` 的判定里，加入「逃逸可能性」的粗判据，把可能逃逸的私有 trait 从 `TranslationUnit(2)`/`LinkageUnit(1)` 回退。**初版可粗**：例如「存在 public 函数/关联项能构造或返回该 trait 的 trait object」→ 不给比 `Public`/`LinkageUnit` 更激进的值。宁可误保守。
- **涉及**：`compiler/rustc_codegen_llvm/src/debuginfo/metadata.rs`。
- **验收**：T2.1/T2.2 全绿；无 miscompile；未过度牺牲明显安全的场景（如纯本地私有 trait 单 CGU 若确实安全可保留）。
- **依赖**：T2.1、T2.2（先有测试再改）。

### T2.4 更新现有 codegen 测试期望 `[S]`
- **做什么**：保守化后，`tests/codegen-llvm/virtual-function-elimination.rs` 里私有 `T`→`!{i64 2}` 等断言可能需改。用 `--emit=llvm-ir` 观察新值后更新，并加注释说明为何变化。
- **验收**：测试绿且断言反映新的保守语义。
- **依赖**：T2.3。

### T2.5 更新 unstable-book 限制说明 `[S]`
- **做什么**：`src/doc/unstable-book/.../virtual-function-elimination.md`：把已修复的场景从「Limitations」移除或改述，明确当前仍存在的边界。
- **验收**：文档与实际行为一致；`./x.py test tidy` 绿。
- **依赖**：T2.3。

---

## 阶段三 · vtable 可达性分析

### T3.1 设计可达性判据（设计文档 + 团队对齐）`[M]`
- **做什么**：把 07 的风险清单转成一套「输入 → 保守 VCallVisibility」的规则草案；在 issue #68262 / Zulip `t-compiler`/`wg-llvm` 征求意见。
- **验收**：有书面判据草案 + 至少一次团队反馈。
- **依赖**：阶段二完成（安全网就位）。

### T3.2 复用/评估现有可见性能力 `[M]`
- **做什么**：调研 `rustc_privacy` 的 effective visibility / reachability、coherence 查询（枚举 impl、识别 blanket impl）、mono item collection 能否直接支撑判据；能复用就别造轮子。
- **涉及**：`rustc_privacy`、`rustc_trait_selection`/coherence、`rustc_monomorphize`。
- **验收**：产出「可复用能力清单 + 缺口清单」。
- **依赖**：T3.1。

### T3.3 实现可达性 query `[L]`
- **做什么**：按 T3.1/T3.2 新增一个 query（在 `rustc_middle/src/queries.rs` 声明 + 在合适 crate 写 provider），输出保守 `VCallVisibility`；让 `apply_vcall_visibility_metadata` 改为调用它。
- **涉及**：`rustc_middle`、provider 所在 crate、`rustc_codegen_llvm/.../metadata.rs`（改为消费 query）。
- **验收**：阶段二所有回归测试仍绿；在可证明安全的场景能提升可见性；新增覆盖「安全提升」的正向测试。
- **依赖**：T3.1、T3.2。

### T3.4 限定 LLVM 后端 / 处理 GCC `[S]`
- **做什么**：确保新逻辑与 `type_checked_load` 只在 LLVM 后端生效；GCC 后端（占位实现，`rustc_codegen_gcc/src/intrinsic/mod.rs:685`）不被误导（必要时诊断或跳过）。
- **验收**：GCC 后端不受影响、不 miscompile。
- **依赖**：T3.3。

---

## 阶段四 · 让 WPD 可观测

### T4.1 WPD 正向去虚化测试 `[M]`
- **做什么**：构造「唯一可行 impl + fat LTO + 安全可见性」，断言最终 IR/asm 里间接调用被去虚化（教程 10.6）。可用 `tests/assembly-llvm/` 或 run-make。
- **验收**：测试证明去虚化真实发生；且在保守场景下**不**发生（负向对照）。
- **依赖**：T1.4 结论；理想情况下 T3.3 提供安全可优化场景。

### T4.2 （条件性）排查 LTO pass 配置 `[M/L]`
- **做什么**：若 T1.4/T4.1 发现 WPD 未真正触发，调查 `compiler/rustc_codegen_llvm/src/back/lto.rs` 的 pass pipeline，定位为何 WPD 没跑。
- **验收**：给出根因；若可行则修复使 WPD 生效。
- **依赖**：T4.1（发现问题时才需要）。

---

## 阶段五 · ThinLTO 后续（可选）

### T5.1 放开并验证 ThinLTO `[L]`
- **做什么**：评估并放开 `Lto::Thin` 在校验（`session.rs:1557`）、元数据（`metadata.rs:1699`）、checked load（`meth.rs:136`）三处的限制；处理 summary 传播、whole-program-visibility 标志；按 `metadata.rs:1731-1732` FIXME 重新评估 ThinLTO 的 VCallVisibility 取值。
- **验收**：ThinLTO 下正确且有测试；不 regress fat LTO。
- **依赖**：阶段二~四稳固。

---

## 建议的 PR 切分

| PR | 内容 | 对应任务 |
| --- | --- | --- |
| PR-1 | 保守化 + 北极星回归（修 miscompile） | T2.1, T2.3, T2.4, T2.5（+T1.*为前置调研） |
| PR-2 | 更多逃逸场景回归 | T2.2 |
| PR-3 | 可达性 query（核心） | T3.1–T3.4 |
| PR-4 | WPD 可观测测试（+可能的 pipeline 修复） | T4.1（, T4.2） |
| PR-5+ | ThinLTO | T5.1 |

> PR-1 最小、争议最小、价值明确（修官方承认的 miscompile），是理想的**第一个 PR**。

## 本篇小结

- 五阶段共约 15 个任务，均带验收标准与依赖。
- 关键路径：T1.4（WPD 是否真触发）→ T2.*（保守化+回归，第一个 PR）→ T3.*（可达性 query，核心）→ T4.*（可观测）。
- 建议按 PR-1..PR-5 切分，从最小可交付的正确性修复起步。

下一篇：[`06-testing-strategy.md`](06-testing-strategy.md)
