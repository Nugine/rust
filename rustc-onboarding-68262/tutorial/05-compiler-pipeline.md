# 05 · 编译流水线

目标：把「源码到 LLVM IR」这条路上的每个阶段讲清楚，并用贯穿全教程的例子 `a.noise()` 一路跟踪。读完你应该能指出「#68262 的工作发生在流水线的哪一段」（剧透：主要在**单态化 + codegen**这一末段）。

贯穿例子（回顾）：

```rust
trait Animal { fn noise(&self) -> u32; }
struct Cat;
impl Animal for Cat { fn noise(&self) -> u32 { 1 } }
fn make(a: &dyn Animal) -> u32 { a.noise() }
```

## 5.1 全流程一览

```
源码 .rs
  │  ① 词法 (rustc_lexer / rustc_parse)
  ▼ tokens
  │  ② 语法分析
  ▼ AST (rustc_ast)
  │  ③ 宏展开 (rustc_expand)
  ▼ 展开后的 AST
  │  ④ 名称解析 (rustc_resolve)
  ▼ 带绑定的 AST
  │  ⑤ 降低 lowering (rustc_ast_lowering)
  ▼ HIR (rustc_hir)
  │  ⑥ 类型检查 + trait 求解 (rustc_hir_analysis / typeck / trait_selection)
  ▼ 带类型信息的 HIR / Ty
  │  ⑦ MIR 构建 (rustc_mir_build)
  ▼ MIR (rustc_middle::mir)
  │  ⑧ 借用检查 + MIR 变换/优化 (rustc_borrowck / rustc_mir_transform)
  ▼ 优化后的 MIR
  │  ⑨ 单态化 / 收集 (rustc_monomorphize)
  ▼ mono items + CGU 划分
  │  ⑩ codegen (rustc_codegen_ssa → rustc_codegen_llvm)
  ▼ LLVM IR (含 vtable global + 元数据)
  │  ⑪ LLVM 优化 (含 WholeProgramDevirt) + 生成目标码
  ▼ .o
  │  ⑫ 链接 (+ LTO)
  ▼ 可执行文件 / 库
```

**#68262 的战场是 ⑨⑩⑪**：单态化决定哪些 vtable 存在、被谁用；codegen 决定生成什么元数据；LLVM 阶段是这些元数据真正被消费、做去虚化的地方。前面 ①-⑧ 你基本只需要「知道它们存在」。

下面逐段说明，重点段落展开，非重点段落一句带过。

## 5.2 ①-④ 前端：从文本到有绑定的语法树

- **① 词法**：`fn make ( a : & dyn Animal ) ...` 被切成 token。
- **② 语法**：token 组装成 AST 节点：一个 `Fn`，参数类型是 `&dyn Animal`，函数体里有个方法调用 `a.noise()`。
- **③ 宏展开**：本例没有宏，跳过。真实代码里 `println!` 之类在这里展开。
- **④ 名称解析**：把标识符 `Animal`、`Cat`、`noise`、`a` 绑定到它们的定义。此时 rustc 知道 `Animal` 是个 trait、`noise` 是它的方法。

这几步的产物对本任务几乎透明——你不会在这里改代码。

## 5.3 ⑤ HIR：High-level IR

AST 被「降低」成 **HIR**（High-level Intermediate Representation）。HIR 比 AST 更规整、去掉了一些语法糖，是类型检查的输入。

对例子：`make` 成为一个 HIR item，`a.noise()` 是一个 HIR 方法调用表达式，`&dyn Animal` 是一个 HIR 类型。**每个 HIR 节点都有一个 `HirId`，每个 item 都有一个 `DefId`。**

## 5.4 ⑥ 类型检查 + trait 求解（Rust 语义的心脏）

这一步做两件对本任务概念上重要的事：

1. **类型检查**：确定 `a: &dyn Animal`，`a.noise()` 的返回类型是 `u32`，`make` 返回 `u32`。
2. **trait 求解**：确定 `noise` 是通过 `dyn Animal` 这个 **trait object** 调用的——也就是说这是一次**动态派发（dynamic dispatch）**，要走 vtable，而不是静态调用 `Cat::noise`。

> 关键概念（教程 07 详讲）：`dyn Animal` 是一个 **existential type / trait object**；`&dyn Animal` 是一个「胖指针」，由「数据指针 + vtable 指针」两部分组成。「要不要 vtable」在这里就定了。

trait 求解还负责 **coherence / orphan 规则**——即「哪些 crate 能给 `Animal` 实现 impl」。这直接关系到 #68262 里「候选实现集合是否封闭」的判断（roadmap/07 会展开）。

## 5.5 ⑦⑧ MIR：Mid-level IR（去虚化决策的语义基础）

HIR 被降低成 **MIR**（Mid-level IR）——一种基于「基本块 + 语句 + 终结符」的控制流图，接近机器但仍带 Rust 语义。借用检查、许多优化、以及常量求值都在 MIR 上做。

对例子，`a.noise()` 在 MIR 里大致变成：一个 `Call` 终结符，其被调用者是「**从 `a` 的 vtable 里取出第 N 个 slot 得到的函数指针**」。也就是说，「虚调用」在 MIR 层就已经是「从 vtable 取函数再 call」的显式形态。

> 对本任务：你通常**不改 MIR**，但要理解「虚调用在 MIR 里就是一次 vtable 取指 + 间接 call」，因为 codegen 正是照着 MIR 这个形态去生成 LLVM 的 `llvm.type.checked.load` + 间接 call。

## 5.6 ⑨ 单态化与收集（决定「有哪些 vtable」）

Rust 的泛型是**单态化**的：`Vec<u8>` 和 `Vec<i32>` 会各生成一份代码。`rustc_monomorphize` 从入口出发，**收集**所有实际会被用到的具体实例（函数、vtable、静态量），这些叫 **mono items**，再把它们**划分到若干 codegen 单元（CGU, codegen unit）**。

对本任务，这一段极其关键，因为它决定了：

- **某个 `(Cat, dyn Animal)` 的 vtable 是否会被生成**（只有被用到才生成）；
- **这个 vtable 被划到哪个 CGU、被哪些函数引用**；
- 进而影响「这个 vtable 的使用范围有多大」——是只在一个 CGU 内，还是跨多个 CGU，还是可能被下游 crate 引用。

调查报告里提到的「single_cgu」判断、「TranslationUnit vs LinkageUnit」区别，根子都在这一段的 CGU 划分语义上。

## 5.7 ⑩ codegen：MIR → LLVM IR（本任务的落点）

`rustc_codegen_ssa` 遍历 MIR，把它翻译成一组**后端无关**的抽象操作（通过 `BuilderMethods` 等 trait）；`rustc_codegen_llvm` 实现这些 trait，真正调用 LLVM C++ API 产出 IR。

对例子，这一段会：

1. 为 `Cat as Animal` 生成一个 **vtable global**（LLVM 常量数组：drop 指针、size、align、然后是 `noise` 的函数指针）。
2. 给这个 vtable global 挂上 **`!type` 元数据**（typeid 来自 `rustc_symbol_mangling` 的 v0 mangling）和 **`!vcall_visibility` 元数据**（可见性，来自 `apply_vcall_visibility_metadata`）。
3. 在 `a.noise()` 的调用点，若开启了 VFE + fat LTO，用 **`llvm.type.checked.load`** intrinsic 从 vtable 取函数指针，而不是普通 load——这样 LLVM 才能把「这个调用点」和「那些带 `!type` 的 vtable」关联起来。
4. 设置模块级 flag `"Virtual Function Elim"`。

**这四件事就是 #68262 现有实现的全部 rustc 侧机制**，也是你要动的地方。它们的具体代码坐标在 roadmap/03 有逐条清单。

## 5.8 ⑪⑫ LLVM 优化与链接（去虚化真正发生的地方）

LLVM 拿到带元数据的 IR 后，在 **LTO（链接时优化）** 阶段运行 `WholeProgramDevirt` / `GlobalDCE` 等 pass：

- 读 `!type`：找出「某个虚调用点可能命中哪些 vtable / 哪些函数」。
- 读 `!vcall_visibility`：判断这个候选集合**是否完整**。只有当 LLVM 相信「已经看到全部可能的实现」时，才敢把间接调用替换成直接调用（进而内联、常量传播、删死代码）。
- 如果 rustc 给的可见性**过于乐观**，LLVM 会误以为候选集合完整，删掉/内联实际仍会被外部调用的函数 → **miscompile**。

这就是为什么 #68262 的核心难点在 rustc 侧的**可见性判断保守性**，而不是 LLVM 侧的 pass 本身（pass 已经存在且能用）。

## 5.9 把例子从头到尾串一遍

| 阶段 | `a.noise()` / `Cat: Animal` 变成了什么 |
| --- | --- |
| 词法/语法 | 一个方法调用表达式，参数类型 `&dyn Animal` |
| 类型检查/trait 求解 | 确定是对 `dyn Animal` 的**动态派发**，走 vtable |
| MIR | 「从 `a` 的 vtable 取 slot N → 间接 call」 |
| 单态化/收集 | 生成 `Cat as Animal` 的 vtable mono item，划入某 CGU |
| codegen | vtable global + `!type` + `!vcall_visibility`；调用点发 `llvm.type.checked.load` |
| LLVM/LTO | 若可见性允许，`WholeProgramDevirt` 把间接 call 变直接 call 并内联 |

## 5.10 本篇小结

- 流水线 12 段，本任务只关心末段：**⑨ 单态化 → ⑩ codegen → ⑪ LLVM/LTO**。
- 「是否虚调用」在类型检查/trait 求解定；「有哪些 vtable、范围多大」在单态化/CGU 划分定；「生成什么元数据」在 codegen 定；「是否真去虚化、是否 miscompile」在 LLVM/LTO 定。
- rustc 侧能改的、也是本任务要改的：**vtable 元数据的正确性与保守性**。

## 延伸阅读

- rustc-dev-guide「Overview / The compiler pipeline」：`src/doc/rustc-dev-guide/src/overview.md`
- MIR 章节：`src/doc/rustc-dev-guide/src/mir/index.md`
- 单态化/monomorphization：`src/doc/rustc-dev-guide/src/backend/monomorph.md`
- 在线：<https://rustc-dev-guide.rust-lang.org/part-3-intro.html>

下一篇：[`06-core-datastructures.md`](06-core-datastructures.md) —— 深入 `TyCtxt`、query、`DefId`、`Ty` 这些你在 codegen 代码里天天见到的东西。
