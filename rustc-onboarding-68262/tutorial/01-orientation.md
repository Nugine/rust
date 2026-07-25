# 01 · 定位与心智模型

目标：在写任何代码之前，先建立对 rustc 的整体认知——它是什么、由什么组成、和 LLVM 怎么分工、为什么它被叫做「查询驱动（query-based）」的编译器。这一篇不涉及具体代码坐标，是纯粹的「地图」。

## 1.1 rustc 是什么

`rustc` 是 Rust 的官方编译器。它把 `.rs` 源码翻译成目标平台的机器码（或库）。但和很多教学用编译器不同，rustc 是一个**产品级、增量式、以正确性和诊断质量为第一目标**的大型系统：

- 它非常在意**错误信息质量**（大量代码是为了给出好的 diagnostics）。
- 它是**增量编译**的：改一行代码重新编译时，它会尽量复用上次的结果。这直接塑造了它的架构（见 1.4 的 query 系统）。
- 它把「翻译到机器码」这一步**外包给后端**，默认后端是 **LLVM**。

对本任务（#68262）来说，关键认知是：**rustc 自己并不实现「去虚化」这个优化，LLVM 才实现。rustc 的职责是生成正确的 LLVM IR 和元数据，让 LLVM 的去虚化 pass 能安全地工作。** 这条分工线贯穿整个任务，一定要记牢。

## 1.2 前端 / 中端 / 后端

可以用经典的三段式来给 rustc 分层，虽然边界没那么绝对：

- **前端（frontend）**：词法分析、语法分析、名称解析、宏展开。产出 AST → HIR。
- **中端（middle）**：类型检查、trait 求解、借用检查、把 HIR 降低（lower）成 MIR，并在 MIR 上做分析与优化。这是 rustc「最 Rust」的部分——所有权、生命周期、trait coherence 都在这里。
- **后端（backend）**：单态化（monomorphization，把泛型实例化成具体类型），再把 MIR 翻译成后端 IR。默认后端把 MIR 翻成 **LLVM IR**，交给 LLVM 生成机器码。

rustc 支持多个 codegen 后端：
- `rustc_codegen_llvm`（默认，基于 LLVM）
- `rustc_codegen_gcc`（基于 GCC 的 libgccjit）
- `rustc_codegen_cranelift`（基于 Cranelift，主要用于快速 debug 构建）

它们共享一个「后端无关」的骨架 `rustc_codegen_ssa`。**#68262 的改动主要落在 `rustc_codegen_ssa`（后端无关部分）和 `rustc_codegen_llvm`（LLVM 专属部分）**，这个分层在教程 08 会详细讲。

## 1.3 rustc 与 LLVM 的分工（对本任务至关重要）

把这张分工图刻进脑子：

```
Rust 源码
   │  rustc 前端 + 中端
   ▼
  MIR（Rust 自己的中间表示）
   │  rustc 后端：单态化 + codegen
   ▼
 LLVM IR  ──────────────►  LLVM 优化 pass（包括 WholeProgramDevirt / VFE）
   │                                   │
   │                                   ▼
   └───────────────────────────►  机器码 / 目标文件
```

- **rustc 生成 LLVM IR**：包括函数体、vtable（作为 LLVM global 常量）、以及挂在这些 IR 对象上的**元数据（metadata）**，比如 `!type`、`!vcall_visibility`。
- **LLVM 消费这些元数据**去做去虚化：LLVM 的 `WholeProgramDevirt.cpp` 会读 `!type` 找出「某个虚调用点可能调用哪些函数」，再结合 `!vcall_visibility` 判断「这个候选集合是否完整、能不能安全地静态化」。
- **关键风险**：如果 rustc 给 LLVM 的元数据「过于乐观」（比如声称某 vtable 只在本模块内使用，但实际会被下游 crate 调用），LLVM 会删掉/内联它以为用不到的东西，导致**错误编译（miscompile）**。#68262 的难点正是「如何让 rustc 保守而正确地生成这些可见性元数据」。

所以你会发现：本任务里「写 Rust 编译器代码」的比重，可能不如「理解 Rust 的可见性/trait/跨 crate 语义，并把它翻译成 LLVM 能理解的保守元数据」。

## 1.4 「查询驱动」的编译器（query system）

这是 rustc 和传统编译器最不一样的地方，也是读源码时最容易懵的地方，值得单独讲。

**传统编译器**通常是「一趟趟 pass 顺序跑」：先全量做词法，再全量做语法，再全量类型检查……每一步产出喂给下一步。

**rustc 是「按需拉取」的**：几乎所有中端/后端信息都被建模成**查询（query）**。一个 query 就像一个「带缓存的纯函数」：

- 你问 `tcx.type_of(def_id)`（这个定义的类型是什么？），
- 它算一次、把结果**缓存**起来，
- 下次再问同一个问题直接返回缓存。

这个「带缓存的纯函数」网络是**惰性、去重、可增量**的：只有真正被需要的信息才会被计算；增量编译时，没变化的 query 结果可以跨编译会话复用。

几个你马上会遇到的名字（细节在教程 06）：

- **`TyCtxt<'tcx>`**（读作 "the tcx"）：编译期的「上帝对象 / 中央上下文」。几乎所有 query 都是它的方法，比如 `tcx.visibility(def_id)`、`tcx.type_of(def_id)`。你在 codegen 代码里会看到它无处不在。
- **`DefId`**：对「某个定义」（一个 fn、一个 struct、一个 trait……）的全局唯一标识。很多 query 的输入就是 `DefId`。
- **query**：上面说的带缓存的函数。#68262 里，一个可能的产出就是「**新增一个 query，用来保守判断某个 vtable 的使用可见性**」——见 roadmap/04 阶段三。

> 为什么你需要现在就知道 query？因为当调查报告说「不要继续在 `tcx.visibility(trait_def_id)` 上打补丁，而应定义一个更接近 vtable-use-reachability 的分析」时，它说的「分析」大概率就是「一个新 query」。理解 query 系统，你才读得懂那句话。

## 1.5 一次编译大致发生了什么（鸟瞰）

把贯穿全教程的例子 `a.noise()` 放进这个流程里，先建立时间线，细节留给后面：

1. **驱动（driver）**：`rustc_driver` 启动，解析命令行，建立编译会话 `Session`（记录 `-Z` 开关、目标平台、LTO 模式等）。#68262 相关的 `-Zvirtual-function-elimination`、`-Clto=fat` 都存在这里。
2. **前端**：源码 → token → AST → 宏展开 → 名称解析 → HIR。此时 `dyn Animal`、`a.noise()` 还只是语法结构。
3. **中端**：类型检查确定 `a: &dyn Animal`、`noise` 的签名；trait 求解确定 `Cat: Animal`；HIR 降低成 MIR。在 MIR 里，`a.noise()` 变成「通过 vtable 做一次间接调用」的形式。
4. **单态化**：收集实际用到的泛型实例和 vtable，形成 codegen 单元（CGU）。vtable 是否会被生成、被谁用，在这里初步确定。
5. **codegen**：把 MIR 翻成 LLVM IR。`rustc_codegen_ssa` 里 `get_vtable` / `load_vtable` 负责 vtable 的创建和虚调用点的翻译；`rustc_codegen_llvm` 负责真正发出 LLVM 的 global、intrinsic 和元数据。
6. **LLVM**：优化（可能包含去虚化）+ 生成目标文件。
7. **链接**：把目标文件、依赖、运行时链成最终产物。LTO（链接时优化）就发生在这一带，是「whole program」信息可用的时机。

## 1.6 本篇小结

- rustc = 前端（Rust 语义）+ 后端（翻译到 LLVM IR），**优化和去虚化由 LLVM 做**。
- #68262 的本质是：**让 rustc 生成正确且保守的 LLVM 元数据**，从而让 LLVM 的去虚化在安全前提下生效。
- rustc 是查询驱动的：`TyCtxt` + `DefId` + query（带缓存的纯函数）是理解一切的钥匙。
- 「新增一个保守的可见性分析」在 rustc 里往往等价于「新增一个 query」。

## 延伸阅读

- rustc-dev-guide「Overview of the compiler」：`src/doc/rustc-dev-guide/src/overview.md`
- rustc-dev-guide「Queries: demand-driven compilation」：`src/doc/rustc-dev-guide/src/query.md`
- 在线版：<https://rustc-dev-guide.rust-lang.org/overview.html>

下一篇：[`02-repo-layout.md`](02-repo-layout.md) —— 我们把上面这些抽象层，对应到仓库里具体的目录和 crate。
