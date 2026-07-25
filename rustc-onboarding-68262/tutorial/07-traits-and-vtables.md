# 07 · trait、`dyn Trait` 与 vtable（核心概念篇）

目标：把本任务最核心的语言机制讲透——什么是 trait object、胖指针长什么样、vtable 的内存布局、`dyn Trait` 怎么从具体类型「变胖」（unsizing coercion）、以及「虚调用」在 rustc 里怎么表示。这是理解 #68262 的地基，值得读慢一点。

## 7.1 静态派发 vs 动态派发

同样调用 `noise()`，有两种派发方式：

```rust
fn stat<T: Animal>(a: &T)      -> u32 { a.noise() } // 静态派发：单态化后直接 call Cat::noise
fn dynamic(a: &dyn Animal)     -> u32 { a.noise() } // 动态派发：运行时通过 vtable 找函数再 call
```

- **静态派发**：`T` 在编译期已知，单态化后 `a.noise()` 变成直接调用 `Cat::noise`，可内联。**没有 vtable，也不需要去虚化**。
- **动态派发**：`a` 是 trait object `&dyn Animal`，编译期不知道背后是 `Cat` 还是别的类型，只能运行时查 vtable。**这才是 #68262 想优化的对象**：在「其实只有一种可能实现」等条件下，把这个动态调用变回直接调用。

## 7.2 trait object `dyn Animal` 是什么

`dyn Animal` 是一个 **unsized（DST，动态大小）** 类型：你不能直接持有一个 `dyn Animal` 值（它大小未知），只能通过**胖指针**间接持有：`&dyn Animal`、`Box<dyn Animal>`、`*const dyn Animal` 等。

在 rustc 内部，`dyn Animal` 是 `Ty` 的 `TyKind::Dynamic` 形态（教程 06），它携带一组「existential predicates」，其中最重要的是 **principal trait ref**——也就是 `Animal` 这个主 trait。这个 principal 决定了：

- vtable 里有哪些方法项；
- 生成 `!type` 元数据用的 **typeid**（由 principal trait ref 经 v0 mangling 得到）。

代码里从一个类型里取出「principal dyn trait」的辅助函数就是 `meth.rs` 用到的 `dyn_trait_in_self`（`meth.rs:139` 附近调用），它返回一个 `ExistentialTraitRef`。

## 7.3 胖指针的内存布局

一个 `&dyn Animal` 在内存里是**两个机器字**：

```
&dyn Animal  =  ┌─────────────┬─────────────┐
                │ data 指针    │ vtable 指针  │
                └─────────────┴─────────────┘
                  指向真实的      指向 Cat 的
                  Cat 值          vtable 常量
```

- **data 指针**：指向真实数据（这里是一个 `Cat`）。
- **vtable 指针**：指向一个**编译期生成的常量表**（vtable），描述「`Cat as Animal` 的方法都在哪」。

调用 `a.noise()` 时，运行时做的事：**从 vtable 指针 → 取出 `noise` 对应的 slot → 得到函数指针 → 以 data 指针为 `self` 调用它**。「取出 slot」这一步，就是 `meth.rs::load_vtable` 干的事。

## 7.4 vtable 的布局

vtable 是一段常量，由 `tcx.vtable_entries(trait_ref)`（教程 06 的 `VtblEntry` 列表）决定，布局大致为：

```
索引  内容（VtblEntry）
 0    MetadataDropInPlace   ← drop glue 函数指针（怎么析构这个具体类型）
 1    MetadataSize          ← 具体类型的大小
 2    MetadataAlign         ← 具体类型的对齐
 3    Method(Cat::noise)    ← 第一个虚方法
 4    Method(...)           ← 更多虚方法 / 父 trait 指针 / Vacant 空槽
...
```

- 前三项（drop/size/align）是所有 vtable 共有的，即 `COMMON_VTABLE_ENTRIES`（`ty/vtable.rs:45` 附近）。
- 之后是方法项 `Method(Instance)`——**去虚化想静态化的就是这些**。
- 可能还有 `TraitVPtr`（指向父 trait 的 vtable，用于 trait 继承）和 `Vacant`（占位）。

> 与 LLVM 的对接：整个 vtable 被生成为一个 LLVM global 常量，rustc 在它身上挂 `!type`（typeid，offset=0）和 `!vcall_visibility`。调用点用 `llvm.type.checked.load(vtable, byte_offset, typeid)`，其中 `byte_offset` 精确指向上面某个 `Method` 项的字节偏移。

## 7.5 unsizing coercion：从 `&Cat` 到 `&dyn Animal`

「变胖」这个动作叫 **unsizing coercion**。当你把 `&Cat` 赋给 `&dyn Animal` 时：

```rust
let c = Cat;
let a: &dyn Animal = &c;  // 这里发生 unsize coercion：&Cat -> &dyn Animal
```

编译器在这一点：

1. 确定需要 `Cat as Animal` 的 vtable；
2. **触发这个 vtable 被生成**（单态化 collector 会把它记为一个 mono item）；
3. 构造胖指针 `(data=&c, vtable=&VTABLE_for_Cat_as_Animal)`。

**这个 coercion 发生在哪里、能不能逃逸到下游 crate，正是 #68262 可见性判断的关键**：如果某个 public、`#[inline]` 或泛型函数里发生了这个 coercion，那么下游 crate 就可能拿到并调用这个 vtable，即使 `Animal` trait 本身是私有的。「trait 私有」不代表「vtable 用不到外面去」。

## 7.6 谁能给 `Animal` 加 impl？——coherence / orphan 规则

去虚化要静态化一个虚调用，前提之一是**知道候选实现集合是封闭的**（「我看到的 impl 就是全部可能的 impl」）。Rust 靠 **coherence / orphan 规则**约束「谁能写 impl」：

- 大体上，`impl Animal for X` 只能写在「`Animal` 所属 crate」或「`X` 所属 crate」里。
- 但 **blanket impl**（`impl<T: Bound> Animal for T`）、泛型 impl、以及下游 crate 用本地类型实现上游 trait 等情况，会让「候选集合」在你编译本 crate 时**并不封闭**——下游还能新增实现。

对本任务：**存在 blanket/泛型 impl，或 trait 可被下游实现时，候选集合不封闭，就不能安全缩窄 `!vcall_visibility`**。这条规则决定了大量「必须保守」的边界（roadmap/07 有清单）。

## 7.7 「虚调用」在 rustc 里的真实形态

把前面串起来，`a.noise()` 的动态派发在 codegen 里对应两段代码（都在 `rustc_codegen_ssa/src/meth.rs`）：

1. **`get_vtable`（`meth.rs:103`）**：需要某个 `(ty, trait_ref)` 的 vtable 时，
   - 先查缓存；
   - 用 `tcx.vtable_allocation((ty, trait_ref))` 拿到 vtable 的常量数据；
   - `static_addr_of` 生成 LLVM global；
   - **`cx.apply_vcall_visibility_metadata(ty, trait_ref, vtable)`** ← 挂可见性/type 元数据（本任务核心函数）；
   - 生成 debuginfo、写缓存。

2. **`load_vtable`（`meth.rs:126`）**：从 vtable 里取某个 slot 的函数指针时，
   - **若 `-Zvirtual-function-elimination` 且 `Lto::Fat`**：取出 principal trait ref → `typeid_for_trait_ref` 生成 typeid → 调 `bx.type_checked_load(llvtable, vtable_byte_offset, typeid)`（对应 LLVM `llvm.type.checked.load`）；
   - **否则**：退化为普通 `load`（带 invariant / nonnull 元数据）。

这两段就是 rustc 侧「让 LLVM 能看懂虚调用结构」的全部机制。**注意**：`type_checked_load` 只有 LLVM 后端实现；GCC 后端目前是占位（本任务要限定 LLVM，见 roadmap/03）。

## 7.8 `!vcall_visibility` 到底怎么被决定的（现状）

`apply_vcall_visibility_metadata`（`rustc_codegen_llvm/src/debuginfo/metadata.rs:1691`）当前逻辑（已核对源码）：

1. 若未开 VFE 或不是 `Lto::Fat`，**直接返回**（不挂任何元数据）。
2. 取 principal `trait_ref`，`trait_def_id = trait_ref.def_id`，`trait_vis = tcx.visibility(trait_def_id)`。
3. 取 CGU 数量，`single_cgu = (cgus == 1)`。
4. 用 `(lto, trait_vis, single_cgu)` 三元组匹配，得到 `Public(0) / LinkageUnit(1) / TranslationUnit(2)`：
   - 无 LTO + public，或无 LTO + 私有 + 多 CGU → `Public`；
   - 有 LTO + public，或有 LTO + 私有 + 多 CGU → `LinkageUnit`；
   - 私有 + 单 CGU → `TranslationUnit`。
5. 挂 `!type`（typeid，offset=0）和 `!vcall_visibility`。

**这就是调查报告说「用 `tcx.visibility(trait_def_id)` 近似」的确切代码**。#68262 的核心改动，就是把第 2、4 步换成一个考虑「vtable 是否真会被外部调用」的更保守判断。

## 7.9 本篇小结

- 动态派发 = 胖指针（data + vtable）+ 运行时查 vtable；静态派发无 vtable。
- `dyn Animal` = `TyKind::Dynamic`，其 principal trait ref 决定 vtable 方法项和 typeid。
- vtable 布局 = drop/size/align（共有三项）+ 方法项 `Method(Instance)`（去虚化目标）。
- unsize coercion 是 vtable「诞生并可能逃逸」的地方；trait 私有 ≠ vtable 用不到外面。
- coherence/blanket impl 决定候选集合是否封闭——不封闭就必须保守。
- 现状：`meth.rs` 的 `get_vtable`/`load_vtable` + `metadata.rs` 的 `apply_vcall_visibility_metadata` 三处，构成 rustc 侧全部机制；可见性用 `tcx.visibility(trait_def_id)` 近似。

## 延伸阅读

- rustc-dev-guide「Trait solving」与「Coherence」章节：`src/doc/rustc-dev-guide/src/traits/`
- Rust Reference「Trait objects」：<https://doc.rust-lang.org/reference/types/trait-object.html>
- 源码：`compiler/rustc_middle/src/ty/vtable.rs`、`compiler/rustc_codegen_ssa/src/meth.rs`

下一篇：[`08-codegen-and-llvm.md`](08-codegen-and-llvm.md) —— 把 codegen 的分层（ssa vs llvm）、intrinsic、给 global 挂 metadata 的机制讲清楚，直达本任务改动点。
