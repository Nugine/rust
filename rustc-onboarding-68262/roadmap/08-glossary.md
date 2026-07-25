# 08 · 术语表

目标：一站式解释本手册（教程 + roadmap）用到的术语。按主题分组，带上「在本任务里为什么重要」。

## LLVM / 优化侧

**WPD（Whole-Program Devirtualization，整程序去虚化）**
LLVM 在 LTO 时把「实际会被调用的间接虚调用」替换成直接调用（进而内联等）。**#68262 的真正目标。** 见 roadmap/01、02。

**VFE（Virtual Function Elimination，虚函数消除）**
LLVM 删除 vtable 里「从不被动态调用」的函数项。PR #96285 已实现其基础设施（`-Zvirtual-function-elimination`）。**VFE ≠ WPD**：前者删未用项，后者静态化仍用的调用。

**devirtualization（去虚化）**
把动态派发（经 vtable 的间接调用）转成静态派发（直接调用）的统称。

**`!type` metadata（type metadata / type identifier）**
挂在 vtable global 上的 `!{offset, typeid}`，把 vtable 归到某个类型标识下，让 LLVM 收集「同一 trait 的所有 vtable」。见 roadmap/02.2。

**typeid**
`!type` 里那段字符串，由对 existential trait ref 做 v0 mangling 得到（`typeid_for_trait_ref`）。同一 trait ref → 同一 typeid。

**`!vcall_visibility`**
挂在 vtable 上的整数 `0/1/2` = `Public/LinkageUnit/TranslationUnit`，告诉 LLVM 这个 vtable 的虚调用可能来自多大范围。**本任务风险的集中点**：标太窄 → miscompile。

**`llvm.type.checked.load(vtable, offset, typeid)`**
LLVM intrinsic，显式表达「从某 typeid 的 vtable 的某 offset 取函数指针」，让调用点对去虚化透明。返回 `{ptr, i1}`，rustc 只用 ptr。见教程 08.4。

**module flag `"Virtual Function Elim"`**
LLVM VFE/GlobalDCE 的开关兼安全阀（`MergeBehavior::Error`）。见教程 08.6。

**LTO（Link-Time Optimization，链接时优化）**
- **fat LTO**：把所有对象合并成一个大模块再优化，能看到整个链接单元。**本任务现状只支持它。**
- **ThinLTO**：分模块 + 摘要协作，不合并；VFE/WPD 支持更复杂。roadmap 阶段五。
- **ThinLocal**：单 crate 内的 thin LTO 形态（判定表里出现）。

**GlobalDCE**
LLVM 的全局死代码消除 pass，VFE 依附其上。

## Rust 语言 / 类型侧

**trait object / `dyn Trait`**
运行时多态类型，unsized，只能经胖指针持有。内部是 `TyKind::Dynamic`，带 principal trait ref。见教程 07.

**胖指针（fat/wide pointer）**
`&dyn Trait` = `(data 指针, vtable 指针)` 两个机器字。

**vtable（virtual method table，虚方法表）**
编译期生成的常量表，描述某 `具体类型 as Trait` 的方法地址等。布局 = drop/size/align + 方法项。rustc 内部是 `&[VtblEntry]`（`ty/vtable.rs`）。

**`VtblEntry`**
vtable 一项：`MetadataDropInPlace`/`MetadataSize`/`MetadataAlign`/`Vacant`/`Method(Instance)`/`TraitVPtr(TraitRef)`。**`Method` 是去虚化目标。**

**unsizing coercion**
`&Cat → &dyn Animal` 的「变胖」动作，是 vtable 诞生并可能逃逸的地方。见教程 07.5。

**existential trait ref / principal**
`dyn Trait` 里那个「主 trait」的引用（`ExistentialTraitRef`），决定 vtable 方法项与 typeid。

**coherence / orphan 规则**
约束「谁能为某 trait 写 impl」的规则。决定候选实现集合是否封闭——**不封闭就不能安全去虚化**。见教程 07.6、roadmap/07 R5。

**blanket impl**
形如 `impl<T: Bound> Trait for T {}` 的泛型 impl，使候选集合对下游开放（不封闭）。roadmap/07 R4。

**monomorphization（单态化）**
把泛型按具体类型实例化，各生成一份代码。决定「哪些 vtable 被生成、被谁用」。见教程 05.6。

**Instance**
单态化后的「具体某个函数」= `DefId` + 具体泛型参数。

## rustc 架构侧

**`TyCtxt<'tcx>`（the tcx）**
编译期中央上下文，几乎所有 query 是它的方法。见教程 06.1。

**query**
带缓存的纯函数（惰性、去重、可增量），声明在 `rustc_middle/src/queries.rs`。**「新增分析」通常 = 「新增 query」。**

**`DefId` / `LocalDefId`**
定义的全局标识 / 本 crate 内定义。本地 vs 外部之分是跨 crate 风险之源。

**`Ty` / `TyKind`**
类型句柄 / 其枚举形态（`Dynamic` = `dyn Trait`）。

**`Visibility`（`ty/mod.rs:386`）**
`Public` / `Restricted(mod)`。当前用来近似 `!vcall_visibility`，但表达的是**名字可见性**而非**使用可达性**。roadmap/07.1。

**effective visibility / reachability（`rustc_privacy`）**
「实际能从外部到达」的分析，比 `Visibility` 更接近本任务所需语义。roadmap/07 Q2。

**CGU（Codegen Unit，代码生成单元）**
单态化后代码的分组，影响并行度与 `!vcall_visibility` 的 TranslationUnit 判定。`-Ccodegen-units=N` 控制数量。

**codegen_ssa vs codegen_llvm**
后端无关骨架（走 trait）vs LLVM 专属发射。本任务改动分布在两者。见教程 08.1。

**`apply_vcall_visibility_metadata`**
`rustc_codegen_llvm/src/debuginfo/metadata.rs:1691` 的函数，计算并挂 `!vcall_visibility` + `!type`。**本任务核心改动点。**

**mono item collection**
单态化时「收集实际用到的实例/vtable」的过程，可判断某 vtable 是否只在本 CGU 用。roadmap/07 Q1。

## 构建 / 流程侧

**x.py / bootstrap**
构建入口脚本 / 其背后的 Rust 构建编排器（`src/bootstrap`）。见教程 03。

**stage0 / stage1 / stage2**
下载的种子编译器 / 用它编出的（含你改动的）编译器 / 完全自举的编译器。日常用 stage1。

**compiletest**
`tests/` 的测试驱动器，按 `//@` 指令编译+断言。

**FileCheck**
LLVM 工具，按 `// CHECK:` 断言 IR/文本形态。codegen 测试用它。

**run-make**
可多 crate、真链接、真运行的端到端测试。**本任务正确性守护主力。**

**bors**
rust-lang/rust 的合并机器人；`@bors r+` 入队、合并前逐 PR 全量 CI。

**tidy**
代码规范/许可证/文档一致性静态检查（`./x.py test tidy`）。

**ICE（Internal Compiler Error）**
编译器自身 panic。用 `RUST_BACKTRACE=1` + debug assertions 定位。

**`[TRACKED]`**
`-Z` 开关标记，表示它影响增量编译缓存 key（`virtual_function_elimination` 即 TRACKED）。

## 关键引用

- issue #68262：<https://github.com/rust-lang/rust/issues/68262>
- PR #96285：<https://github.com/rust-lang/rust/pull/96285>
- LLVM Type Metadata：<https://llvm.org/docs/TypeMetadata.html>
- rustc-dev-guide：<https://rustc-dev-guide.rust-lang.org/>（本仓库 `src/doc/rustc-dev-guide/`）
- 前置调查报告：[`../../issue-68262-investigation.md`](../../issue-68262-investigation.md)

—— roadmap 部分到此结束。回到 [`00-index.md`](00-index.md) 或 [`../README.md`](../README.md)。
