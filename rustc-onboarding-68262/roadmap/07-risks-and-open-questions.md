# 07 · 风险与未决问题

目标：把所有「一不小心就 miscompile」的边界，以及需要团队决策的开放问题，集中成一份可逐条核对的清单。设计可见性判据时（阶段三），每条都要能回答「我的判据对这个场景保守吗？」。

## 7.1 核心命题：可见性 ≠ 可达性

反复强调，因为它是所有风险的根源：

> `tcx.visibility(trait_def_id)` 回答「这个 trait 的**名字**能否在外部被命名」。本任务真正需要的是「这个 vtable 的虚调用**可能来自哪里**（使用可达性）」。**名字私有的 trait，其 vtable 仍可能被外部 crate 调用。** 用前者近似后者，就是当前实现会 miscompile 的原因。

## 7.2 miscompile 风险清单（逐条）

每条给出：场景 → 为什么危险 → 保守要求。

### R1. 私有 trait 经 public wrapper 逃逸（官方承认）
- **场景**：私有 `trait Foo`；`pub struct FooBox(Box<dyn Foo>)`；`pub fn make_foo() -> FooBox`。
- **危险**：下游 crate 能拿到包着 `dyn Foo` 的值并调用其方法，尽管 `Foo` 名字私有。
- **保守要求**：只要存在 public 路径能**构造/返回**该 trait 的 trait object，就不能按「本 crate 私有」缩窄可见性。

### R2. 私有 trait 经 `#[inline]` 函数逃逸（官方承认）
- **场景**：`#[inline] pub fn f(a: FooBox) { a.0.foo() }`。
- **危险**：`#[inline]` 函数体会被**内联到下游 crate**，于是 `Foo::foo` 的虚调用实际发生在下游 crate。
- **保守要求**：若某 public `#[inline]`（或泛型，见 R3）函数体内对该 trait object 有虚调用，视为可跨 crate 调用。

### R3. 私有 trait 经泛型函数逃逸
- **场景**：`pub fn g<T>(x: &dyn Foo)`（或返回/存储 `dyn Foo` 的泛型 API）。
- **危险**：泛型函数在下游单态化，其中对私有 trait object 的调用被复制到下游。
- **保守要求**：同 R2，public 泛型函数体内的相关虚调用视为可跨 crate。

### R4. blanket impl / 泛型 impl 使候选集合不封闭
- **场景**：`impl<T: Bound> Foo for T {}`。
- **危险**：编译本 crate 时，无法枚举全部可能的实现类型——下游还能带来新的 `T`。候选集合不封闭，去虚化不能假设「只有这些实现」。
- **保守要求**：存在 blanket/泛型 impl 时，默认 `Public`，不做「唯一实现」假设。

### R5. 下游可为 trait 添加 impl（coherence/orphan）
- **场景**：trait 或类型允许下游写新的 `impl`。
- **危险**：候选实现集合在本 crate 编译时不封闭。
- **保守要求**：结合 orphan 规则判断「下游能否新增该 trait 的 impl」；能则不封闭。注意 blanket impl、local type + foreign trait 等组合需细分。

### R6. 多 CGU 下的 TranslationUnit 判定
- **场景**：`-Ccodegen-units>1`，一个私有 vtable 被多个 CGU 引用。
- **危险**：`TranslationUnit(2)` 意味着「只本翻译单元可见」，但多 CGU 下 vtable 可能跨 CGU 使用。
- **保守要求**：单 CGU 只是允许 `TU` 的**必要非充分**条件；跨 CGU 使用时不得标 `TU`。（现状用 `single_cgu` 作近似，需重新审视。）

### R7. 动态链接边界（cdylib / dylib / exported symbols）
- **场景**：产物是 cdylib/dylib，或有导出符号；LTO 只覆盖部分程序。
- **危险**：「whole program」假设不成立——动态库另一侧的代码可能调用该 vtable，但不在本次 LTO 视野内。
- **保守要求**：跨动态链接边界、可导出的 vtable 保守处理（倾向 `Public`），即使 fat LTO。`LinkageUnit` 的语义是「链接单元内」，不含动态库另一侧。

### R8. 后端不一致（GCC 占位）
- **场景**：GCC 后端的 `type_checked_load` 是占位（返回 0，`rustc_codegen_gcc/src/intrinsic/mod.rs:685`）。
- **危险**：若可见性/去虚化逻辑在非 LLVM 后端被误用，行为未定义。
- **保守要求**：把 VFE/WPD 相关逻辑限定在 LLVM 后端；其他后端不启用。

### R9. 只测元数据、不测最终优化
- **场景**：只有 codegen 测试断言 `!vcall_visibility` 存在。
- **危险**：以为 WPD 生效，实际 pipeline 没跑，或反之——正确性/收益都没被真正验证。
- **保守要求**：补 run-make/assembly 测试验证真实行为（阶段四）。

## 7.3 未决问题（需要调研或团队决策）

这些问题没有现成答案，建议在 issue #68262 / Zulip `t-compiler`/`wg-llvm` 讨论：

### Q1. 判据放在哪一层？
新的「vtable 可达性」判据，应该是 `rustc_middle` 的 query、`rustc_privacy` 的扩展、还是 codegen 阶段的局部计算？涉及缓存/增量/跨 crate 信息可得性。（倾向 query，教程 06。）

### Q2. 能否直接复用 effective visibility / reachability？
`rustc_privacy` 已有「实际可从外部到达」的分析。它是否足以表达「vtable 经 wrapper/inline/泛型逃逸」？还是需要专门的、面向 vtable-use 的分析？需 T3.2 调研。

### Q3. `#[inline]` 与跨 crate 内联的精确边界
哪些函数体会被复制到下游？`#[inline]`、泛型、`const fn`、被 `#[inline(always)]`……需要精确列出「会把虚调用带到下游」的构造集合。

### Q4. `LinkageUnit` 在有导出符号时是否仍安全？
即使 fat LTO，若最终产物导出符号或是 dylib，`LinkageUnit` 的「链接单元」是否等于「whole program」？（R7）

### Q5. 是否需要拆分/重命名 flag？
`-Zvirtual-function-elimination` 名义是 VFE。真正做 WPD 后，是否需要区分「删未用项」与「静态化仍用调用」的语义/开关？（调查报告阶段五提到。）

### Q6. WPD 到底有没有在当前 pipeline 跑？
T1.4/T4.1 要先回答。若没跑，工作重心会从「前端元数据」转向「LTO pass 配置」（`back/lto.rs`）。

### Q7. 性能影响权衡
保守化会减少优化机会（体积/性能可能退步）；放开会增加正确性风险。需要 perf run 数据支撑取舍（教程 11.4）。

## 7.4 「保守回退」判定原则（可直接用作实现兜底）

当阶段三的判据对某场景无法给出确定结论时，按此兜底：

```
if 存在 blanket/泛型 impl 或下游可加 impl:        → Public
elif 存在 public 路径构造/返回该 trait object:    → 不高于 LinkageUnit（且若跨动态链接边界 → Public）
elif public #[inline]/泛型 函数体内有该虚调用:     → 不高于 LinkageUnit
elif 跨 CGU 使用 且 想标 TranslationUnit:         → 降为 LinkageUnit
elif 有导出符号 / cdylib / dylib:                → Public
else 无法证明安全:                               → Public
```

> 这是「安全网」，不是最终精确判据。阶段二可先落地它的粗版；阶段三再用真正的可达性分析把能证明安全的场景从 `Public` 提升上来。

## 7.5 本篇小结

- 一切风险源于「可见性 ≠ 可达性」。
- 九条 miscompile 风险（R1–R9）：wrapper 逃逸、inline 逃逸、泛型逃逸、blanket impl、下游可加 impl、多 CGU、动态链接边界、后端不一致、只测元数据。
- 七个未决问题（Q1–Q7）需调研/团队决策，尤其「判据放哪层」「能否复用 reachability」「WPD 是否真跑」。
- 7.4 的保守回退原则可作实现兜底：**证明不了安全就 `Public`**。

下一篇：[`08-glossary.md`](08-glossary.md)
