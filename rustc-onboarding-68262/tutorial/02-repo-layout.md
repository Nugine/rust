# 02 · 仓库结构与 crate 划分

目标：把上一篇的抽象分层，对应到这个仓库里真实存在的目录和 crate。读完你应该能回答：「要改 vtable 的可见性元数据，我该去哪个目录、哪个 crate？」

所有路径都相对仓库根（本环境是 `/home/runner/work/rust/rust`）。

## 2.1 仓库顶层

```
rust/
├── compiler/     ← rustc 本体：约 75 个 rustc_* crate（我们主要在这里工作）
├── library/      ← 标准库：core / alloc / std / proc_macro 等
├── tests/        ← 编译器测试套件（ui / codegen-llvm / run-make ...）
├── src/          ← 工具、文档、bootstrap 构建系统、内嵌的 llvm-project 子模块
├── x.py / x      ← 构建入口脚本（见教程 03）
├── bootstrap.example.toml  ← 构建配置模板（复制成 bootstrap.toml 使用）
├── Cargo.toml / Cargo.lock ← 整个仓库是一个大 Cargo workspace
└── src/version   ← 当前版本号（本仓库：1.99.0）
```

对本任务，你 99% 的时间在 `compiler/` 和 `tests/` 里。

## 2.2 `compiler/` —— rustc 的 crate 群

rustc 不是一个巨型 crate，而是被切成了约 75 个 `rustc_*` crate（`ls compiler` 可以看到全部）。这样切分主要是为了**编译并行度**和**分层清晰**。它们大致按编译阶段分层。下面只挑与理解本任务相关的，按「前端 → 中端 → 后端」排列：

### 基础设施 / 通用

| crate | 作用 |
| --- | --- |
| `rustc_data_structures` | 各种数据结构、并发原语、哈希表等基础件 |
| `rustc_index` | 类型安全的下标（`newtype_index!`） |
| `rustc_span` | 源码位置 `Span`、`Symbol`（字符串驻留） |
| `rustc_macros` | rustc 内部用的过程宏（如自动派生 query） |
| `rustc_session` | **编译会话 `Session`：命令行选项、`-Z`/`-C` 开关都在这里定义**。`-Zvirtual-function-elimination` 定义在 `rustc_session/src/options.rs` |
| `rustc_target` | 目标平台（triple）、ABI、target spec |
| `rustc_abi` | 类型的布局/ABI 抽象（`Layout` 等） |

### 前端

| crate | 作用 |
| --- | --- |
| `rustc_lexer` | 纯词法分析（无依赖，可独立使用） |
| `rustc_parse` | 语法分析：token → AST |
| `rustc_ast` | AST 定义 |
| `rustc_expand` / `rustc_builtin_macros` | 宏展开 |
| `rustc_resolve` | 名称解析（把标识符绑定到定义） |
| `rustc_ast_lowering` | AST → HIR 的降低 |
| `rustc_hir` | HIR 定义 |

### 中端（Rust 语义的核心）

| crate | 作用 |
| --- | --- |
| `rustc_middle` | **最重要的 crate**：定义 `TyCtxt`、`Ty`、MIR、大量 query 的签名。几乎所有后续 crate 都依赖它 |
| `rustc_hir_analysis` / `rustc_hir_typeck` | 类型检查 |
| `rustc_infer` / `rustc_trait_selection` / `rustc_next_trait_solver` / `rustc_traits` | 类型推断与 trait 求解（判断 `Cat: Animal` 之类） |
| `rustc_privacy` | **可见性 / 隐私分析**——本任务判断「谁能看到/用到某个 vtable」时可能要用到这里的 effective-visibility |
| `rustc_mir_build` / `rustc_mir_transform` / `rustc_mir_dataflow` | MIR 的构建、变换（优化）、数据流分析 |
| `rustc_borrowck` | 借用检查 |
| `rustc_ty_utils` | 一些类型相关的 query 实现（比如 vtable 布局的计算就在这一带） |
| `rustc_monomorphize` | **单态化 + collect：决定哪些函数/ vtable 会被实际生成，分配到哪个 CGU** |

### 后端（codegen）

| crate | 作用 |
| --- | --- |
| `rustc_codegen_ssa` | **后端无关的 codegen 骨架**：把 MIR 翻成一套抽象的「builder」调用。`get_vtable` / `load_vtable` 在这里（`meth.rs`）。#68262 的一部分改动落在这里 |
| `rustc_codegen_llvm` | **LLVM 专属后端**：实现那些抽象 builder，真正发出 LLVM IR、intrinsic、global 和 metadata。`apply_vcall_visibility_metadata`、`type_checked_load`、module flag 都在这里 |
| `rustc_codegen_gcc` | GCC 后端（不支持 `llvm.type.checked.load`，本任务需注意别误用） |
| `rustc_codegen_cranelift` | Cranelift 后端 |
| `rustc_llvm` | rustc 到 LLVM C++ API 的 FFI 胶水层 |
| `rustc_symbol_mangling` | **符号名 mangling**——vtable 的 `!type` 元数据用的 typeid 就是这里生成的（v0 mangling） |

### 驱动 / 组装

| crate | 作用 |
| --- | --- |
| `rustc_driver` / `rustc_driver_impl` | 命令行入口，串起整个编译流程 |
| `rustc_interface` | 把各阶段组织成可调用的 API |
| `rustc` | 最终的 binary crate（很薄，主要是 `main`） |

> **本任务的「热点 crate」**（记住这四个就够开始了）：
> `rustc_codegen_ssa`、`rustc_codegen_llvm`、`rustc_symbol_mangling`、`rustc_session`。
> 加上判断可见性时可能涉及的 `rustc_middle` / `rustc_privacy` / `rustc_monomorphize`。

## 2.3 `library/` —— 标准库

`core`、`alloc`、`std` 等。本任务基本不改标准库，但你需要知道：
- 测试里的 `dyn Trait`、`Box<dyn Trait>` 等会用到 std 的类型；
- 构建 rustc 时也会构建 std（见教程 03 的 stage 概念）。

## 2.4 `tests/` —— 测试套件

这是本任务**大量工作量所在**（roadmap/06 会专门讲）。与本任务相关的子目录：

| 目录 | 类型 | 与本任务的关系 |
| --- | --- | --- |
| `tests/ui/` | UI 测试：编译并比对 stdout/stderr（或只要求 build-pass） | `tests/ui/codegen/virtual-function-elimination.rs` 在此 |
| `tests/codegen-llvm/` | codegen 测试：编译到 LLVM IR，用 FileCheck 断言 IR 形态 | `virtual-function-elimination.rs`、`virtual-function-elimination-32bit.rs` 在此，**本任务核心测试** |
| `tests/assembly-llvm/` | 断言最终汇编 | 可能用于验证去虚化后的实际调用 |
| `tests/run-make/` | 用 Makefile/rust 驱动的端到端测试，可编译多 crate、跑链接 | **跨 crate、LTO 场景**很可能要在这里写 |
| `tests/ui-fulldeps/` | 依赖完整 rustc 内部库的测试 | 潜在用于跨 crate 场景 |
| `tests/codegen-units/` | 断言单态化/CGU 划分 | 理解 CGU 影响 vcall visibility 时有用 |

> 注意目录名是 `codegen-llvm`（不是旧的 `codegen`）。本仓库已经把 LLVM 专属的 codegen 测试放在 `tests/codegen-llvm/`。

## 2.5 `src/` —— 工具、文档、构建系统

| 子目录 | 作用 |
| --- | --- |
| `src/bootstrap/` | **构建系统本体**（`x.py` 背后的 Rust 代码）。教程 03 详解 |
| `src/doc/rustc-dev-guide/` | 官方 rustc 开发指南（子模块），**你的权威参考书** |
| `src/doc/unstable-book/` | 不稳定特性/开关的文档。`compiler-flags/virtual-function-elimination.md` 在此，**本任务已承认的限制写在这里** |
| `src/llvm-project/` | 内嵌的 LLVM 源码子模块（构建 `rustc_codegen_llvm` 需要） |
| `src/tools/` | 各种配套工具：`compiletest`（测试驱动器）、`tidy`（代码规范检查）、`clippy`、`rustfmt` 等 |
| `src/ci/` | CI 配置 |
| `src/version` | 版本号文件 |

## 2.6 怎么快速定位「某个功能在哪个 crate」

三个实用手段（教程 10 会再展开）：

1. **按符号名 grep**：知道函数名/常量名，直接全局搜。例如：
   ```
   grep -rn "apply_vcall_visibility_metadata" compiler/
   ```
2. **按 `-Z` 开关名反查**：开关定义都在 `rustc_session/src/options.rs`，从定义处顺藤摸瓜找消费点。
3. **看 crate 名猜层**：名字即职责，`rustc_codegen_*` 一定是后端，`rustc_privacy` 一定是可见性。

## 2.7 本篇小结

- 仓库 = `compiler/`（rustc）+ `library/`（std）+ `tests/`（测试）+ `src/`（工具/文档/bootstrap/LLVM）。
- rustc 被切成约 75 个 `rustc_*` crate，按编译阶段分层。
- **本任务热点**：`rustc_codegen_ssa`、`rustc_codegen_llvm`、`rustc_symbol_mangling`、`rustc_session`，可见性分析可能牵扯 `rustc_middle`/`rustc_privacy`/`rustc_monomorphize`。
- 测试重点在 `tests/codegen-llvm/`、`tests/ui/`、`tests/run-make/`。

## 延伸阅读

- rustc-dev-guide「High-level compiler architecture」与「The compiler source code」：`src/doc/rustc-dev-guide/src/part-2-intro.md` 一带
- 在线：<https://rustc-dev-guide.rust-lang.org/compiler-src.html>

下一篇：[`03-build-system.md`](03-build-system.md) —— 怎么把这一大堆 crate 真正构建成一个能跑的 rustc。
