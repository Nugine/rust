# 02 · LLVM 背景

目标：把 LLVM 侧的四个机制（`!type`、`!vcall_visibility`、`llvm.type.checked.load`、`"Virtual Function Elim"` module flag）以及去虚化 pass 怎么协作讲清楚。理解这些，你才知道 rustc 发的每一样东西「LLVM 拿去干嘛」，进而明白哪里错了会 miscompile。

（教程 08 从 rustc 侧讲「怎么发」；本篇从 LLVM 侧讲「怎么用」。）

## 2.1 大局：LLVM 如何做去虚化

LLVM 在 **LTO** 阶段跑 `WholeProgramDevirt`（WPD）和 `GlobalDCE`（含 VFE）两类 pass。它们要回答两个问题：

1. **某个虚调用点，可能命中哪些函数？**（候选集合）
2. **这个候选集合是否完整/封闭？**（能不能安全静态化或删除）

问题 1 靠 **`!type`** 回答：把「调用点」和「带相同 typeid 的 vtable」关联起来。
问题 2 靠 **`!vcall_visibility`** 回答：告诉 LLVM「这个 vtable 的虚调用可能来自多大范围」。

只有当 LLVM 相信「我看到了全部候选」时，才敢：把间接调用换成直接调用（WPD），或删掉没被调用的 vtable 项（VFE）。**rustc 的责任就是只在真的安全时才让 LLVM 相信这一点。**

## 2.2 `!type` 元数据：给 vtable 归类

LLVM 的「type metadata」机制（<https://llvm.org/docs/TypeMetadata.html>）允许给一个 global 打上一个或多个 **type identifier**。形式是 `!{offset, typeid}`：

- rustc 给每个 vtable global 挂 `!{i64 0, !"<mangled typeid>"}`（offset 恒为 0）。
- **typeid** 来自对 existential trait ref 做 v0 mangling（教程 08 的 `typeid_for_trait_ref`）。**同一个 trait ref → 同一个 typeid**，于是所有 `X as Animal` 的 vtable 都属于同一类。
- offset=0 对应 Itanium ABI 里 vtable「address point」的建模方式（PR #96285 提交说明有解释）。

有了 `!type`，LLVM 扫描所有带某 typeid 的 vtable，就得到「实现了这个 trait 的所有 vtable」候选集。

## 2.3 `!vcall_visibility`：候选集合是否封闭

这是本任务的核心。`!vcall_visibility` 是挂在 vtable 上的一个整数，取值：

| 值 | LLVM 名称 | 含义 | 允许 LLVM 做什么 |
| --- | --- | --- | --- |
| 0 | Public | 可能被任意外部代码使用 | 最保守，基本不能删/静态化 |
| 1 | LinkageUnit | 只在当前**链接单元**内可见 | LTO 后可认为候选完整，能优化 |
| 2 | TranslationUnit | 只在当前**翻译单元**内可见 | 更激进，能做 TU 级删除/静态化 |

**语义要点**：值越大，LLVM 越敢优化，rustc 越需要确信「没有该范围之外的代码会用这个 vtable」。

- 标 `2`（TranslationUnit）= rustc 向 LLVM 保证「这个 vtable 只可能被本翻译单元用」。
- 标 `1`（LinkageUnit）= 保证「只可能被本次链接的所有对象用」（fat LTO 下即整个程序，但不含跨动态库边界）。
- 标 `0`（Public）= 不做保证，LLVM 保守处理。

**miscompile 的根源**：如果 rustc 标了 `2` 或 `1`，但实际上该 vtable 会被**该范围之外**（比如下游 crate 内联进来的调用、动态库另一侧）使用，LLVM 会误删/误静态化，导致运行时调用到被删函数或错误目标。

## 2.4 `llvm.type.checked.load`：让调用点可被识别

普通 `load ptr from vtable+offset` 对 LLVM 去虚化是**不透明**的：LLVM 不知道这个 load 出来的指针是「某 typeid 的 vtable 的某 slot」。

`llvm.type.checked.load(vtable, byte_offset, typeid)` 显式表达了这一点，返回 `{ptr, i1}`：

- `ptr`：从 `vtable + byte_offset` 取出的函数指针；
- `i1`：类型检查是否通过（rustc 只用 ptr，见教程 08）。

有了它，LLVM 才能把「这个调用点」和「typeid 对应的候选 vtable 集」连起来，进而在 WPD 里判断「所有候选在这个 offset 上的函数是否相同/唯一」，若唯一就直接调用它。

## 2.5 module flag `"Virtual Function Elim"`：总开关 + 安全阀

rustc 用 `add_module_flag_u32(..., MergeBehavior::Error, "Virtual Function Elim", 1)` 设一个模块标志：

- 它是 LLVM VFE/GlobalDCE 的**开关**：没有它，LLVM 不会基于 `!vcall_visibility` 做删除。
- `MergeBehavior::Error`：LTO 合并多个模块时，若它们对该 flag 取值不一致，**链接报错**而非静默产生不一致优化——一个防止「部分模块开、部分关」导致错误的安全阀。

## 2.6 四件套如何协作（一张图）

```
rustc 发出：
  vtable global  --!type {0, typeid}--------------┐
                 --!vcall_visibility {0|1|2}----┐  │
  call site      --llvm.type.checked.load(...)--│--│--┐
  module         --flag "Virtual Function Elim"-│--│--│
                                                │  │  │
LLVM LTO 阶段：                                  ▼  ▼  ▼
  1. 用 !type 收集某 typeid 的全部 vtable  ←──────┘  │  │
  2. 用 checked.load 定位调用点属于哪个 typeid+offset ┘  │
  3. 用 !vcall_visibility 判断候选是否封闭 ←────────────┘
  4. 若封闭且唯一 → WPD 直接调用 / VFE 删未用项
```

**只要第 3 步的 `!vcall_visibility` 是 rustc 保守正确给出的，整条链就安全。本任务几乎全部风险都集中在这一格。**

## 2.7 fat LTO vs ThinLTO（为什么现状只支持 fat）

- **fat LTO**：把所有对象合并成一个大模块再优化，LLVM 能看到「整个链接单元」的全部 vtable 和调用点，`!vcall_visibility` 的语义清晰。
- **ThinLTO**：分模块 + 摘要（summary）协作优化，不合并成单模块。VFE/WPD 在 ThinLTO 下需要额外的 summary 传播和 "whole-program visibility" 标志，能力和正确性更复杂。

rustc 现状（校验、元数据、checked load）都要求 `Lto::Fat`。**本任务先只做 fat LTO**，ThinLTO 放到最后（04 阶段五）。

## 2.8 本篇小结

- LLVM 去虚化要回答「候选是谁」（靠 `!type`）+「候选是否封闭」（靠 `!vcall_visibility`）。
- `!vcall_visibility` 值越大越激进；标错（过窄）= miscompile 根源，是本任务风险集中点。
- `llvm.type.checked.load` 让调用点对去虚化透明；module flag 是开关兼安全阀。
- 现状只支持 fat LTO（语义清晰），ThinLTO 复杂，最后再说。

## 参考

- LLVM Type Metadata：<https://llvm.org/docs/TypeMetadata.html>
- LLVM `WholeProgramDevirt.cpp`：<https://github.com/llvm/llvm-project/blob/main/llvm/lib/Transforms/IPO/WholeProgramDevirt.cpp>
- ThinLTO VFE RFC 讨论：<https://discourse.llvm.org/t/rfc-for-porting-vfe-to-thinlto/73817>

下一篇：[`03-current-state-map.md`](03-current-state-map.md)
