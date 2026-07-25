# 01 · 问题陈述

目标：把 #68262 到底要解决什么、当前进展到哪、以及「算解决」的标准，讲到没有歧义。

参考原始 issue：<https://github.com/rust-lang/rust/issues/68262>；相关 PR：<https://github.com/rust-lang/rust/pull/96285>。

## 1.1 issue 想要什么

原始诉求：让 rustc 支持类似 Clang `-fwhole-program-vtables` 的能力——在 LTO 场景下，让 LLVM 的**去虚化（devirtualization）** 把一部分 `dyn Trait` 的**间接虚调用**转换成**直接调用**，从而解锁内联、常量传播、死代码消除等后续优化。

换句话说，目标是**性能优化**：对下面这种代码，

```rust
fn make(a: &dyn Animal) -> u32 { a.noise() }
```

当整程序信息（LTO）表明 `a` 背后实际只可能是某一种/少数几种实现时，把 `a.noise()` 的间接调用变成对具体函数的直接调用甚至内联。

## 1.2 VFE 与 WPD：两个相关但不同的优化

这是理解本任务**最重要**的区分。它们共享一部分 LLVM 元数据，但目标不同：

| | VFE（Virtual Function Elimination） | WPD（Whole-Program Devirtualization） |
| --- | --- | --- |
| 做什么 | **删除** vtable 里「从不被动态调用」的函数项 | 把「实际会被调用」的间接虚调用**替换成直接调用** |
| 收益 | 减小体积、删死代码 | 内联、常量传播、去间接跳转 |
| rustc 现状 | **已由 PR #96285 实现基础设施**（`-Zvirtual-function-elimination`） | **尚未真正落地/验证** |
| 共享 | `!type`、`!vcall_visibility`、`llvm.type.checked.load`、module flag | 同左 |

**关键**：PR #96285 合入了 `-Zvirtual-function-elimination`，实现了 VFE 所需的 IR 形态，但 issue 后续讨论明确指出**这不应关闭 #68262**——因为 VFE ≠ WPD。issue 真正要的是 WPD（把仍会被调用的虚调用静态化），而不仅是删掉没用的项。

## 1.3 当前进展（PR #96285 交付了什么）

已经存在于 rustc 的机制（03 有精确坐标）：

1. vtable global 上的 `!type` 元数据（typeid 来自 v0 mangling）；
2. vtable global 上的 `!vcall_visibility` 元数据（0/1/2）；
3. 虚调用点用 `llvm.type.checked.load` 而非普通 load；
4. LLVM module flag `"Virtual Function Elim"`；
5. `-Zvirtual-function-elimination` 开关，且校验必须搭配 fat LTO；
6. 64/32 位 codegen 测试 + 一个 build-pass ui 测试。

**但存在两个缺口**：
- **正确性缺口**：`!vcall_visibility` 用 `tcx.visibility(trait_def_id)` 近似，会在私有 trait 逃逸时过度乐观 → 可能 miscompile（unstable-book 已承认，见 1.5）。
- **目标缺口**：没有测试证明「真正的 WPD 去虚化」在安全条件下发生；现有测试只验证「元数据形态」和「VFE 删未用函数」。

## 1.4 为什么这个问题难

难点**不在 LLVM 侧**（去虚化 pass、intrinsic 都已具备），而在 **rustc 侧的语义判断**：

> 如何在 Rust 的「可见性 + trait 一致性（coherence）+ 泛型 + `#[inline]` + 跨 crate + 动态链接」模型下，**保守且正确**地判断「某个 vtable 的虚调用可能来自哪里、候选实现集合是否封闭」，从而决定能否安全地给它标更窄的 `!vcall_visibility`。

标错（过窄）→ LLVM 删/静态化了实际仍被外部调用的函数 → miscompile。标对但太保守 → 只是少了优化，安全。**所以本任务本质是一个「保守正确性」工程，而非「多发几条 LLVM 指令」的工程。**

## 1.5 已被官方承认的 miscompile 例子

`src/doc/unstable-book/src/compiler-flags/virtual-function-elimination.md` 的「Limitations」一节给了确切反例（原文已核对）：

```rust
trait Foo { fn foo(&self) { println!("foo") } }
impl Foo for usize {}

pub struct FooBox(Box<dyn Foo>);
pub fn make_foo() -> FooBox { FooBox(Box::new(0)) }

#[inline]
pub fn f(a: FooBox) { a.0.foo() }
```

推理链：`Foo` 是**私有** trait → 现状假设「它的函数只在本 crate 被看到/调用」→ 若本 crate 没直接调用就可能被优化掉。**但** `make_foo` 是 `pub`，能在**外部 crate** 造出包着 `dyn Foo` 的 `FooBox`；`f` 是 `#[inline] pub`，会被内联到**外部 crate**，于是 `Foo::foo` 实际在外部 crate 被动态调用。此时把它当「本 crate 私有、可删」就会 **miscompile**。

**这个例子是本任务的「北极星测试用例」**——你的保守化改动应当让这种情形不再被过度优化，并有测试守护它。

## 1.6 「算解决 #68262」的验收标准

综合 issue 讨论，一个负责任的收尾至少要满足：

1. **正确性**：不存在「私有 trait 经 public/inline/泛型逃逸而被过度删除/去虚化」的 miscompile；有回归测试覆盖 1.5 这类场景。
2. **真正的 WPD 可观测**：有测试证明在满足安全条件（如唯一可行 impl + fat LTO）时，间接虚调用确实被去虚化为直接调用/内联（不只是元数据存在）。
3. **保守判据**：`!vcall_visibility` 的决定基于「vtable 使用可达性」而非仅「trait 名字可见性」，无法证明安全时回退 `Public`。
4. **文档一致**：unstable-book 的限制说明与实际行为一致（修好的场景从「已知限制」移除，未修的明确列出）。
5. **范围明确**：清楚声明支持边界（fat LTO、LLVM 后端），ThinLTO/GCC 的现状与后续计划有交代。

> 注意：完整关闭 issue 可能超出单个 PR。更现实的路径是「一系列 PR」，第一批做正确性与测试（见 04/05）。

## 1.7 本篇小结

- 目标 = LTO 下把间接虚调用静态化（WPD），性能优化。
- VFE（删未用项，已实现）≠ WPD（静态化仍用的调用，未落地）。
- 难点在 rustc 侧「保守正确地判断 vtable 使用边界」，不在 LLVM 侧。
- 已有官方承认的 miscompile 例（私有 trait + public wrapper + `#[inline]`），作为北极星用例。
- 验收 = 正确性 + WPD 可观测 + 基于可达性的判据 + 文档一致 + 范围明确。

下一篇：[`02-llvm-background.md`](02-llvm-background.md)
