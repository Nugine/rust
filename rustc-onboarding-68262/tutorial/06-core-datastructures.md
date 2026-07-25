# 06 · 核心数据结构

目标：认识你在 codegen 代码里天天会碰到的几个核心抽象——`TyCtxt`、query、`DefId`、`Ty`、`Instance`、`Visibility`、以及 vtable 的内部表示 `VtblEntry`。这一篇是读懂 roadmap/03 代码地图的前置。

> 所有坐标以本仓库当前状态为准，遇到漂移用符号名 grep。

## 6.1 `TyCtxt<'tcx>`：中央上下文

`TyCtxt`（读 "the tcx"）是编译期的中心枢纽，定义在 `compiler/rustc_middle/src/ty/context.rs`（`pub struct TyCtxt<'tcx>` 约在 `:677`）。它：

- 持有整个编译会话的类型信息、query 缓存、arena 分配器等；
- 提供几乎所有 query 作为它的方法：`tcx.type_of(def_id)`、`tcx.visibility(def_id)`、`tcx.vtable_entries(trait_ref)` ……
- 到处以 `tcx: TyCtxt<'tcx>` 或 `cx.tcx()` 的形式出现。

那个 `'tcx` 生命周期参数是 rustc 的招牌：几乎所有中端/后端的类型（`Ty<'tcx>`、`TraitRef<'tcx>`、MIR 等）都借用 `TyCtxt` 里 arena 分配的数据，所以带着 `'tcx`。你写代码时一般照抄这个生命周期即可，不用深究。

## 6.2 query：带缓存的纯函数

上一篇说过 rustc 是查询驱动的。具体地：

- **query 的声明**集中在 `compiler/rustc_middle/src/queries.rs`（历史上叫 `query/mod.rs`，本仓库已重构到 `queries.rs`；配套修饰符在 `query/modifiers.rs`）。用一个 `query 名字(输入) -> 输出 { ... }` 的 DSL 声明。
- 例子（真实存在，可自己 grep 确认）：
  - `query visibility(def_id: DefId) -> ty::Visibility<ModId>`，`queries.rs:2177` 附近，文档写着「Computes the visibility of the provided `def_id`」。
  - `query vtable_entries(key: ty::TraitRef<'tcx>) -> &'tcx [VtblEntry<'tcx>]`，`queries.rs:1569` 附近。
  - `query own_existential_vtable_entries(...)`，`queries.rs:1563` 附近。
- **query 的实现**通常在对应功能 crate 里（比如可见性相关在 `rustc_privacy` / `rustc_ty_utils` 等），通过「provider」注册给 query 系统。
- 你调用时只写 `tcx.visibility(def_id)`，中间的缓存、增量、循环检测由框架处理。

> 对本任务的意义：调查报告建议「新增一个更接近 vtable-use-reachability 的分析」。在 rustc 的世界里，这**大概率就是新增一个 query**——在 `queries.rs` 声明、在某个 crate 里写 provider。你现在就该建立这个直觉。

## 6.3 `DefId` / `LocalDefId`：定义的身份证

- **`DefId`**：全局唯一地标识「一个定义」——一个 fn、struct、trait、impl、const 等。它由 `krate`（哪个 crate）+ `index`（crate 内序号）组成。很多 query 的输入就是 `DefId`。
- **`LocalDefId`**：当前正在编译的 crate 内的定义（`krate == LOCAL_CRATE`）。区分本地/外部很重要，因为**跨 crate 的东西你看不到全部信息**——这正是 #68262 可见性判断的核心张力：一个 vtable 可能被**外部 crate** 引用，而你在本 crate 编译时看不到那些引用。

## 6.4 `Ty<'tcx>` 与 `TyKind`：类型的表示

- `Ty<'tcx>` 是「一个类型」的驻留句柄（interned），定义在 `rustc_middle`（底层 `TyKind` 在 `rustc_type_ir`）。
- `TyKind` 是它的枚举形态：`Bool`、`Int`、`Adt`（struct/enum）、`Ref`（引用）、`Dynamic`（**就是 `dyn Trait`！**）、`FnPtr` 等。
- 对本任务，最相关的是 **`TyKind::Dynamic`**：它表示 trait object `dyn Trait`，内部带一个「existential predicates」列表（其中的 principal 就是那个主 trait，用来生成 vtable typeid）。教程 07 会展开。

## 6.5 `Instance<'tcx>`：单态化后的「具体某个函数」

泛型函数 `fn f<T>()` 本身不是可 codegen 的东西；`f::<u32>` 才是。**`Instance<'tcx>`** 表示「一个具体单态化实例」= `DefId` + 具体泛型参数（`GenericArgs`）。codegen 处理的是 `Instance`，vtable 里存的函数项也对应 `Instance`。你会在 `meth.rs`、单态化 collector 里频繁见到它。

## 6.6 `Visibility`：可见性（本任务关键）

定义在 `compiler/rustc_middle/src/ty/mod.rs`（`pub enum Visibility<Id = ...>` 约在 `:386`）。大致形态：

- `Public`：公开，任何地方可见。
- `Restricted(mod_id)`：受限于某个模块（`pub(crate)`、`pub(in path)`、私有等都归到这类，携带「可见到哪个模块」的信息）。

`tcx.visibility(def_id)` 返回它。**当前 #68262 实现就是拿 `trait_def_id` 的这个 `Visibility` 去近似决定 LLVM 的 `!vcall_visibility`**（`Public` vs `Restricted` → LinkageUnit/TranslationUnit）。

> 为什么这个近似不够（roadmap 会详述）：`Visibility` 描述的是「这个**名字**能不能被外部命名」，而不是「这个 vtable 会不会被外部**调用**」。一个 `Restricted`（私有）的 trait，其 vtable 仍可能通过 public wrapper、`#[inline]`、泛型逃逸到下游 crate 被调用。名字不可见 ≠ 调用不可达。这是全任务最重要的语义区别。

相关的更强工具是 **effective visibility / reachability**（在 `rustc_privacy` 一带），它计算「实际能从外部**到达**的东西」，比单纯的 `Visibility` 更接近本任务需要的语义——roadmap/04 会评估用它。

## 6.7 vtable 的内部表示：`VtblEntry`

定义在 `compiler/rustc_middle/src/ty/vtable.rs`。一个 vtable 是 `&[VtblEntry<'tcx>]`，每个 entry 是：

- `MetadataDropInPlace`：drop glue 指针；
- `MetadataSize` / `MetadataAlign`：大小/对齐；
- `Vacant`：占位空槽；
- `Method(Instance)`：一个虚方法的具体实例（**去虚化想静态化的就是它**）；
- `TraitVPtr(TraitRef)`：指向父 trait 的 vtable（trait 继承时）。

`COMMON_VTABLE_ENTRIES`（`vtable.rs:45` 附近）是所有 vtable 开头共有的三项（drop/size/align）。方法项排在它们之后。`tcx.vtable_entries(trait_ref)` 这个 query 就产出这张表——codegen 照它生成 LLVM 的 vtable 常量。

理解这张表，你才知道「`!type` typeid 附在整个 vtable global 上」「`llvm.type.checked.load(vtable, offset, typeid)` 里的 offset 指向某个 `Method` 项」这些说法具体指什么。

## 6.8 这些结构如何串起本任务

把本任务用到的对象连成一条链：

```
dyn Animal           ← Ty<'tcx> 里的 TyKind::Dynamic，含 principal trait ref
   │ tcx.vtable_entries(trait_ref)
   ▼
[VtblEntry...]        ← drop/size/align + Method(Cat::noise 的 Instance) ...
   │ codegen (meth.rs get_vtable)
   ▼
LLVM vtable global
   │ apply_vcall_visibility_metadata
   │   ├─ tcx.visibility(trait_def_id)  → Visibility::{Public,Restricted}
   │   └─ (+ LTO 模式 + 是否 single CGU)
   ▼
!type + !vcall_visibility 元数据
```

roadmap 的工作，本质就是**把这条链里「`tcx.visibility(trait_def_id)` 决定 `!vcall_visibility`」这一环，换成一个更保守、考虑跨 crate 可达性的判断**。

## 6.9 本篇小结

- `TyCtxt` 是中央上下文，几乎所有信息都通过它的 query 拿。
- query 声明在 `rustc_middle/src/queries.rs`；「新增分析」通常等于「新增 query」。
- `DefId`/`LocalDefId` 区分本地与外部——外部不可见是本任务风险之源。
- `Ty`/`TyKind::Dynamic` 表示 `dyn Trait`；`Instance` 是单态化后的具体函数。
- `Visibility`（`ty/mod.rs:386`）当前被用来近似 `!vcall_visibility`，但它讲的是「名字可见性」而非「调用可达性」——这是核心缺口。
- vtable = `&[VtblEntry]`（`ty/vtable.rs`），方法项是去虚化的目标。

## 延伸阅读

- rustc-dev-guide「The `ty` module: representing types」：`src/doc/rustc-dev-guide/src/ty.md`
- 「Queries」与「Query system internals」：`src/doc/rustc-dev-guide/src/query.md`
- 在线：<https://rustc-dev-guide.rust-lang.org/ty.html>

下一篇：[`07-traits-and-vtables.md`](07-traits-and-vtables.md) —— 把 `dyn Trait`、胖指针、vtable 布局、unsizing coercion 讲透，这是本任务的核心概念篇。
