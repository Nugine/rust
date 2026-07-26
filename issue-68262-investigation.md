# rust-lang/rust#68262 全面调查报告：Support whole program devirtualization

调查对象：<https://github.com/rust-lang/rust/issues/68262>

调查时间：2026-07-25

## 0. 结论摘要

issue #68262 仍处于 open 状态，目标是让 rustc 支持 LLVM 的 whole-program devirtualization（WPD）：在 LTO 场景下，把一部分 `dyn Trait` 的间接虚调用转换为直接调用，从而触发内联、常量传播和死代码删除等后续优化。

当前 rustc 已经通过 PR #96285 合入了一个相关但不完整的实现：`-Zvirtual-function-elimination`。它主要实现了 LLVM Virtual Function Elimination（VFE）所需的 IR 形态：

- vtable 上的 `!type` 元数据；
- vtable 上的 `!vcall_visibility` 元数据；
- 虚函数表加载点的 `llvm.type.checked.load`；
- LLVM 模块标志 `"Virtual Function Elim"`；
- `-Zvirtual-function-elimination` 必须搭配 fat LTO 的命令行校验。

但这还不等于完整解决 #68262。issue 后续评论明确指出：VFE 是“删除未使用的 vtable 函数项”，WPD 是“把实际会被调用的虚函数调用静态化”。当前最核心的缺口不是“怎么发 LLVM 元数据”，而是 rustc 缺少一套足够正确的 vtable 可见性/可达性分析，用来判断什么时候可以安全地给某个 vtable 标记非 public 的 `!vcall_visibility`。错误地把 vtable 标成更窄的可见性会导致 LLVM 删除仍可能被外部 crate、泛型实例化、`#[inline]` 代码或 blanket impl 使用的函数，产生 miscompile。

因此，建议把后续工作拆成两条线：

1. 先做安全性和回归覆盖：为已知危险模式补测试，把当前 `trait_def_id` visibility 近似带来的边界写清楚；
2. 再做真正的 vtable reachability 分析：从 “trait 是否 public” 扩展到 “trait object/vtable 是否能跨 CGU、跨 crate、跨动态链接边界被构造或调用”。

## 1. 背景：issue 想解决什么

#68262 的问题描述提到：LLVM 在 LTO 时有 devirtualization pass，可以把一些间接调用转换为静态、可内联调用；Clang 侧对应能力是 `-fwhole-program-vtables`。issue 创建者希望 rustc 能模拟 Clang 的行为，并收集实现信息。

issue 评论给出的关键线索：

- LLVM 层面需要发出 type metadata，并使用 `llvm.type.test` / `llvm.type.checked.load`。
- 不能简单把 Rust 的 `Visibility` 映射到 `!vcall_visibility`：即使 trait 是 private/restricted，它仍可能通过 public generic、`#[inline]` 函数或 blanket impl 被下游 crate 间接使用。
- PR #96285 合入了 `-Zvirtual-function-elimination`，但后续评论确认它“不应该关闭 #68262”：VFE 和 WPD 不是同一个优化。
- 2024 年评论指出目前没人活跃处理；理论上可行，但缺少正确判断 vtable visibility 的分析。

## 2. LLVM 机制速览

### 2.1 `!type` metadata

LLVM 的 WPD/VFE 会扫描带 `!type` 的 vtable global，把同一个 type identifier 下的所有候选 vtable 聚合起来。Rust 当前把 trait ref 通过 v0 mangling 生成 typeid，附加到 vtable 上：

- 实现位置：`compiler/rustc_codegen_llvm/src/debuginfo/metadata.rs:1742-1746`
- typeid 生成：`compiler/rustc_symbol_mangling/src/lib.rs:145-149`，`compiler/rustc_symbol_mangling/src/v0.rs:142-158`

Rust vtable 这里使用的 offset 是 0；PR #96285 的提交说明解释这是为了匹配 LLVM/Itanium ABI 对“address point”的建模方式。

### 2.2 `!vcall_visibility`

`!vcall_visibility` 告诉 LLVM 某个 vtable 的虚调用使用范围：

| 值 | LLVM 含义 | 粗略解释 |
| --- | --- | --- |
| 0 | Public | 可能被任意外部代码使用，WPD 必须保守 |
| 1 | LinkageUnit | 链接单元内可见，LTO 后可认为候选集合完整 |
| 2 | TranslationUnit | 单个翻译单元内可见，可做更激进删除 |

当前 rustc 的生成逻辑在：`compiler/rustc_codegen_llvm/src/debuginfo/metadata.rs:1691-1750`。

关键点：当前函数开头硬性要求 `-Zvirtual-function-elimination` 且 `Lto::Fat`，否则直接返回（`metadata.rs:1697-1700`）。这意味着 ThinLTO 路径目前没有启用这套 metadata。

### 2.3 `llvm.type.checked.load`

普通 vtable load 对 WPD 不透明；LLVM 需要通过 `llvm.type.checked.load(vtable, offset, typeid)` 识别一个虚调用点属于哪个 typeid 和哪个 vtable slot。

当前 rustc 代码路径：

- `compiler/rustc_codegen_ssa/src/meth.rs:126-156`：`load_vtable` 在 VFE + fat LTO 时尝试提取 dyn trait principal，生成 typeid，然后调用后端的 `type_checked_load`；否则回退普通 load。
- `compiler/rustc_codegen_llvm/src/intrinsic.rs:1048-1063`：LLVM 后端真正发出 `llvm.type.checked.load`，并只取返回结构体的第 0 个元素（函数指针）。

GCC 后端目前不支持该 intrinsic：`compiler/rustc_codegen_gcc/src/intrinsic/mod.rs:685-692` 返回一个占位值，因此后续工作应优先限定 LLVM backend。

### 2.4 LLVM module flag

当前 rustc 在 `compiler/rustc_codegen_llvm/src/context.rs:471-478` 设置 `"Virtual Function Elim"` 模块标志。这是 LLVM GlobalDCE/VFE 使用的开关，也能避免 LTO 单元之间对该开关不一致时静默产生错误优化。

## 3. 当前 rustc 实现地图

| 文件 | 作用 |
| --- | --- |
| `compiler/rustc_session/src/options.rs:2961-2963` | 定义 `-Zvirtual-function-elimination` |
| `compiler/rustc_session/src/session.rs:1557-1561` | 校验该 flag 需要 fat LTO |
| `compiler/rustc_codegen_ssa/src/meth.rs:103-122` | 创建/缓存 vtable，并调用 `apply_vcall_visibility_metadata` |
| `compiler/rustc_codegen_ssa/src/meth.rs:126-156` | vtable slot load；VFE 路径改用 `type_checked_load` |
| `compiler/rustc_codegen_llvm/src/debuginfo/metadata.rs:1691-1750` | 给 vtable 添加 `!type` 和 `!vcall_visibility` |
| `compiler/rustc_codegen_llvm/src/intrinsic.rs:1048-1063` | 生成 `llvm.type.checked.load` |
| `compiler/rustc_codegen_llvm/src/context.rs:471-478` | 添加 LLVM module flag `Virtual Function Elim` |
| `compiler/rustc_symbol_mangling/src/lib.rs:145-149` | `typeid_for_trait_ref` |
| `compiler/rustc_symbol_mangling/src/v0.rs:142-158` | trait ref typeid 的 v0 mangling 实现 |
| `src/doc/unstable-book/src/compiler-flags/virtual-function-elimination.md` | 用户文档和已知限制 |
| `tests/codegen-llvm/virtual-function-elimination.rs` | 64 位 IR golden test |
| `tests/codegen-llvm/virtual-function-elimination-32bit.rs` | 32 位 offset golden test |
| `tests/ui/codegen/virtual-function-elimination.rs` | build-pass 回归测试 |

## 4. 已有测试覆盖了什么

`tests/codegen-llvm/virtual-function-elimination.rs` 编译参数是：

```text
-Zvirtual-function-elimination -Clto -Copt-level=3 -Csymbol-mangling-version=v0
```

它主要检查：

- vtable global 上存在 `!type` 和 `!vcall_visibility`；
- private trait 的 vcall visibility 可为 `2`；
- public trait 的 vcall visibility 可为 `1`；
- dyn trait 方法调用点发出 `@llvm.type.checked.load`；
- 未使用的 vtable 函数名不应出现在最终 IR 中。

这些测试确认“LLVM IR 形态”存在，但没有直接证明 WPD 已把某个实际间接调用改成直接调用，也没有覆盖已知的跨 crate、泛型、inline、wrapper newtype、blanket impl 等安全边界。

## 5. 为什么 #96285 没完全解决 #68262

PR #96285 标题是 “Introduce `-Zvirtual-function-elimination` codegen flag”。它合入了 VFE 所需基础设施，但 issue 后续讨论指出：

- VFE 删除 never dynamically called 的 vtable 函数项；
- WPD 则会把 still-called 的虚调用转换成 direct call；
- 两者共享部分 LLVM 元数据，但优化目标不同。

更重要的是，PR review 已经指出当前使用 trait visibility 近似 vcall visibility 不够准确。review 中给出的反例大意是：private trait 可以被包在 public struct / public constructor 返回的 `Box<dyn Trait>` 里，再通过 public `#[inline]` 函数、泛型函数或函数指针常量在下游 crate 中触发动态调用。此时 trait 本身不是 public，但 vtable slot 仍可能在当前 crate 之外被调用。

unstable book 也保留了类似限制说明：`src/doc/unstable-book/src/compiler-flags/virtual-function-elimination.md:14-37` 明确写到 private trait + public wrapper + inline 函数可能导致函数被过度删除，从而 miscompile。

## 6. 当前最大技术难点：vtable visibility 不是 item visibility

当前代码核心近似是：

- 从 `trait_ref` 得到 `trait_def_id`；
- 调用 `tcx.visibility(trait_def_id)`；
- 再结合 LTO 模式和 CGU 数量决定 `Public` / `LinkageUnit` / `TranslationUnit`。

位置：`compiler/rustc_codegen_llvm/src/debuginfo/metadata.rs:1713-1740`。

这对于简单例子足够，但对 Rust 的真实可达性不够。一个正确分析至少要考虑：

1. trait 是否 public 只是下界，不是充分条件；
2. dyn trait object 是否能作为 public API 的一部分逃逸；
3. 包装类型是否 public，字段是否 private 并不完全决定方法调用是否能跨 crate 发生；
4. `#[inline]`、泛型函数和 const/function pointer 可能把 private trait 上的调用逻辑复制到下游 crate；
5. blanket impl 或泛型 impl 可能让候选实现集合在当前 crate 编译时并不封闭；
6. downstream crate 是否能为该 trait 添加 impl 受 orphan/coherence 规则影响，但 blanket impl、local type、foreign trait/type 组合仍需细分；
7. 多 CGU 下，TranslationUnit 级别比 LinkageUnit 更难安全判断；
8. cdylib/dylib、exported symbol、LTO 边界和动态链接可见性会改变“whole program”的含义。

因此，后续方案不应继续围绕 `tcx.visibility(trait_def_id)` 做小修小补，而应定义一个更接近 “vtable use reachability” 的查询或分析。

## 7. 建议的入手路线

### 阶段一：复现和确认现状

目标是先用最小成本建立观察能力，而不是直接改优化逻辑。

1. 阅读并运行现有测试：
   - `tests/codegen-llvm/virtual-function-elimination.rs`
   - `tests/codegen-llvm/virtual-function-elimination-32bit.rs`
   - `tests/ui/codegen/virtual-function-elimination.rs`
2. 增加本地临时实验用例，分别观察：
   - 简单 private trait + 单 impl + fat LTO；
   - public trait + 单 impl + fat LTO；
   - private trait 经 public wrapper 逃逸；
   - private trait 经 `#[inline]` 函数被下游调用；
   - blanket impl；
   - 多 CGU。
3. 对每个用例输出 LLVM IR，检查三件事：
   - vtable 是否带 `!type`；
   - vtable 是否带预期 `!vcall_visibility`；
   - call site 是否是 `llvm.type.checked.load`，以及后续 LTO 产物中是否真的 devirtualize。

可用命令方向：`./x.py test tests/codegen-llvm/virtual-function-elimination.rs`。完整 rustc 构建成本很高，第一次只建议跑局部 codegen/ui 测试。

### 阶段二：补安全回归测试

优先把已知危险情形固定下来，避免后续实现扩大误优化。

建议新增或扩展 codegen/ui 测试覆盖：

1. unstable book 中的 private trait + public wrapper + inline 函数例子；
2. PR review 中 private trait 通过 `pub struct FooBox(Box<dyn Foo>)` 和 `#[inline] pub fn f(FooBox)` 泄漏的例子；
3. public generic 函数中调用 private trait object 的例子；
4. blanket impl 下不应把 vcall visibility 缩窄的例子；
5. 多 CGU 下 restricted trait 不应轻易标成 `TranslationUnit` 的例子；
6. 跨 crate run-make 或 ui-fulldeps 测试，确认下游 crate 调用不会因 VFE/WPD 过度删除而崩溃或链接失败。

这些测试应先表达“保守正确”，即宁可暂时 `Public` 或不触发优化，也不能误删。

### 阶段三：设计 vtable reachability 分析

建议新增一个概念上独立的分析：输入 `(concrete_ty, existential_trait_ref, crate/lto/codegen context)`，输出保守的 vcall visibility。

初始规则可保守设计：

- public trait：默认 `LinkageUnit` 只在 fat LTO 且不跨动态链接边界时可考虑，否则 `Public`；
- private/restricted trait：如果 dyn trait object、相关 wrapper、相关调用函数可从 public API 或 inline/generic 路径逃逸，则不能用 `TranslationUnit`；
- blanket impl 或泛型 impl：默认 `Public` 或至少不做更激进假设；
- 单 CGU 只是允许 `TranslationUnit` 的必要条件，不是充分条件；
- 无法证明安全时一律回退 `Public`。

可调查的 rustc 现有能力：

- `privacy_access_levels` / effective visibility：判断 item 是否可从外部命名或间接可达；
- coherence/trait impl 查询：枚举本 crate 和 upstream impl，识别 blanket impl；
- MIR/HIR type visitor：扫描 public API、inline 函数体、泛型函数体中是否出现相关 `dyn Trait` 或 vtable-producing coercion；
- mono item collection：确认某个 vtable 或调用点是否只在当前 codegen unit 内使用。

### 阶段四：让 WPD 可观测

现有测试偏“metadata emission”。要真正关闭 #68262，需要有测试证明：在满足安全条件时，LLVM 产物中间接调用消失或目标函数被直接调用/内联。

建议添加一个 codegen test：

- 构造只有一个可行 impl 的 dyn call；
- 使用 fat LTO 和 `-Zvirtual-function-elimination`；
- 让被调用函数有明显可匹配的 IR 行为；
- 检查最终优化后 IR 不再包含间接 call，或者包含直接目标调用/内联后的效果。

如果发现 WPD pass 实际没有在当前 rustc LTO pipeline 触发，则下一步应调查 `compiler/rustc_codegen_llvm/src/back/lto.rs` 的 LTO pipeline 和 LLVM pass 配置，而不是继续改前端 metadata。

### 阶段五：ThinLTO 后续

当前 rustc 明确只支持 fat LTO：

- option 校验要求 `sess.lto() == config::Lto::Fat`；
- metadata 生成也要求 `Lto::Fat`；
- load_vtable 路径也要求 `Lto::Fat`。

ThinLTO 支持不建议作为第一阶段目标。LLVM WPD ThinLTO 和 VFE ThinLTO 的能力、摘要传播和 whole-program-visibility 标志都更复杂。等 fat LTO 的正确性和测试先站稳后，再考虑放开 `Lto::Thin`。

## 8. 可能的最小可行修复方向

如果目标是逐步推进 #68262，而不是一次性完整实现，推荐顺序如下：

1. 不改变优化行为，先补 regression tests，锁定已知 miscompile 风险；
2. 在 `apply_vcall_visibility_metadata` 中把明显不安全的 private/restricted trait 场景回退为 `Public`；
3. 把当前文档中的“may remove vtable functions too eagerly” 从限制描述转化为测试覆盖；
4. 增加 WPD 是否真正 devirtualize 的 codegen 测试；
5. 在测试保护下，引入保守的 vtable reachability 查询；
6. 只在查询能证明安全时标记 `LinkageUnit` 或 `TranslationUnit`；
7. 最后再讨论是否把 flag 语义从 `virtual-function-elimination` 拆分或重命名，以区分 VFE 和 WPD。

## 9. 风险清单

| 风险 | 后果 | 建议 |
| --- | --- | --- |
| 把 private trait 误判为 TU-only | 下游 crate 通过 inline/generic 路径调用被删除函数，miscompile | 新增跨 crate 测试，默认保守回退 |
| blanket impl 未识别 | 候选 vtable 集合不封闭 | blanket/generic impl 默认 `Public` |
| 只测 IR metadata，不测最终优化 | 以为 WPD 生效但实际未生效 | 增加最终 IR devirtualization 检查 |
| ThinLTO 过早启用 | summary/index 行为不完整，难定位 bug | fat LTO 稳定后再做 |
| GCC backend 路径误用 | `type_checked_load` 不支持 | 限定 LLVM backend，或加明确诊断/跳过 |
| 动态链接边界没建模 | LTO 不是完整 whole program | 对 cdylib/dylib/exported symbols 保守处理 |

## 10. 给“不懂 rustc”的阅读顺序

如果从零开始，建议按这个顺序读：

1. issue #68262 的正文和全部评论；
2. PR #96285 的 description、commit message 和 review thread；
3. `src/doc/unstable-book/src/compiler-flags/virtual-function-elimination.md`，先理解当前限制；
4. `tests/codegen-llvm/virtual-function-elimination.rs`，看 rustc 希望生成什么 LLVM IR；
5. `compiler/rustc_codegen_ssa/src/meth.rs`，理解 vtable 和 virtual call load 的 rustc SSA 抽象层；
6. `compiler/rustc_codegen_llvm/src/debuginfo/metadata.rs:1691-1750`，理解 vtable metadata 生成；
7. `compiler/rustc_codegen_llvm/src/intrinsic.rs:1048-1063`，理解 LLVM intrinsic 发出方式；
8. `compiler/rustc_symbol_mangling/src/v0.rs:142-158`，理解 typeid 字符串来源；
9. LLVM Type Metadata 文档和 `WholeProgramDevirt.cpp`，理解 LLVM 端到底消费哪些信息。

## 11. 参考资料

- rust-lang/rust issue #68262: <https://github.com/rust-lang/rust/issues/68262>
- rust-lang/rust PR #96285: <https://github.com/rust-lang/rust/pull/96285>
- LLVM Type Metadata documentation: <https://llvm.org/docs/TypeMetadata.html>
- LLVM WholeProgramDevirt pass: <https://github.com/llvm/llvm-project/blob/main/llvm/lib/Transforms/IPO/WholeProgramDevirt.cpp>
- Clang whole-program-vtables 用户选项背景：<https://clang.llvm.org/docs/UsersManual.html>
- LLVM ThinLTO VFE RFC discussion: <https://discourse.llvm.org/t/rfc-for-porting-vfe-to-thinlto/73817>

## 12. 本次调查的最终判断

#68262 的“入手点”不是从零实现 LLVM intrinsic；这些基础设施已经在 rustc 中存在。真正要解决的是：在 Rust 语言的可见性、trait impl、泛型、inline、跨 crate 和动态链接模型下，保守且正确地判断某个 vtable 的使用边界。

建议第一份正式补丁不要试图直接“开启更多 WPD”，而是先提交测试和更保守的可见性逻辑，修掉文档中已经承认的潜在过度删除问题。之后再逐步引入 reachability 分析，让更多安全场景从 `Public` 提升到 `LinkageUnit` / `TranslationUnit`，最终用实际 devirtualization codegen test 证明 #68262 被解决。

## 13. 实测发现（本次会话，已用本地构建的 stage1 rustc 验证）

在放通 `ci-artifacts.rust-lang.org` 后，本会话成功本地构建了 stage1 rustc（LLVM 22 CI 制品）并对北极星回归测试
`tests/run-make/virtual-function-elimination-cross-crate` 做了实测复现与修复实验，得到一个**修正了此前认知**的关键结论：

**结论：`VCallVisibility` 的取值（`TranslationUnit=2` 还是 `LinkageUnit=1`）并不是本 miscompile 的决定性因素。**

- 现状 `TranslationUnit(2)`（私有 trait + 单 CGU）：`./main` 触发 `SIGILL`（非法指令，exit 132）——复现 miscompile。
- 将该分支改为 `LinkageUnit(1)` 后重建 rustc：**仍然 `SIGILL`**。已用 `--emit=llvm-ir` 确认 vtable 上的
  `!vcall_visibility` 确实从 `!{i64 2}` 变成了 `!{i64 1}`，即改动生效，但运行时崩溃依旧。
- 把 trait 改成 `pub`（从而走 `LinkageUnit(1)` 分支）后：**同样 `SIGILL`**。

原因：在 LLVM 端，`TranslationUnit` 与 `LinkageUnit` **都会启用** VFE（Whole Program Devirtualization 的函数删除）；
只有 `Public(0)` 才会**禁用** VFE。因此 roadmap 中 T2.3 “把逃逸的私有 trait 从 `TranslationUnit(2)` 回退到
`LinkageUnit(1)`” 的处方，**无法满足其自身“无 miscompile”的验收标准**。

**根因（进一步定位）：** 真正丢失的是跨 crate `#[inline]` 虚调用的 `llvm.type.checked.load` 信息。
- 在下游 crate（`main`）里，`#[inline] pub fn f` 的虚调用被内联；实测在 `-Copt-level=0 -Zmir-opt-level=0` 下
  `f` 的独立代码生成**确实**发出了匹配的 `@llvm.type.checked.load(..., i32 24, metadata !"...3foo3Foo")`
  （typeid 与 vtable 的 `!type` 一致）；但在 `-Copt-level=3` 下最终内联体里退化成了**普通 `load`**（见
  `main_ir.ll` 中 `getelementptr ... i64 24` 后紧跟 `load ptr`）。
- 于是进入 fat-LTO 的合并模块里，vtable 带着 VFE 元数据、却没有任何 `type.checked.load` 引用其 `foo` 槽位，
  WPD 据此判定该槽位“无人使用”并删除 `Foo::foo`，普通 `load` 读到被删/陷阱指针 → `SIGILL`。

**对后续修复的影响（修订 roadmap T2.3 的处方）：**
1. 要真正满足“无 miscompile”，逃逸的 vtable 必须降级到 `Public(0)`（禁用 VFE），而**不是** `LinkageUnit(1)`。
   且该问题**对 `pub` trait 同样存在**，不限于私有 trait。
2. “逃逸”判据应做到 per-trait 且保守：候选信号是“存在 `cross_crate_inlinable`（`#[inline]`/泛型）的可达函数，
   其 MIR 里出现了该 `dyn Trait`”。现有单 crate codegen 测试里的 `taking_t/taking_u/taking_v` 都不是
   inline/泛型（非 `cross_crate_inlinable`），故不会被判为逃逸，可保持 `TranslationUnit(2)`/`LinkageUnit(1)` 不变——
   这样既修掉北极星 miscompile，又不动现有 codegen 测试的期望。
3. 更彻底的正解属于 roadmap 阶段四（T4.2）：排查为何 fat-LTO 的 pre-LTO pipeline 会把 `type.checked.load`
   过早降级为普通 `load`，从而让 WPD 在合并后仍能看到跨 crate 内联虚调用的类型测试。

**可复现的本地构建要点（供后续会话）：**
- 需先 `git fetch --unshallow origin`（初始为 2-commit 浅克隆），否则 bootstrap 找不到上游 bors 提交。
- 运行 x.py 时需 `env -u GITHUB_ACTIONS -u CI`，否则 `CiEnv::current()` 误判 `HEAD^1` 为上游提交而对 CI LLVM 404。
- 当前 HEAD 对应的上游 LLVM 制品提交为 `29e68fe2295f8fc2feb52b8cb0b61a055842fdcf`（HTTP 200）。
- 增量重建 `x.py build --stage 1 compiler/rustc` 约 35s；跑单个 run-make：
  `env -u GITHUB_ACTIONS -u CI python3 x.py test tests/run-make/virtual-function-elimination-cross-crate --stage 1`。
