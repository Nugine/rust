# 06 · 测试策略

目标：给出本任务的测试蓝图——每类测试测什么、放哪里、怎么写、怎么保证「有区分度」。本任务的**第一份价值就是测试**，所以这一篇要具体到能照着落地。（测试机制本身见教程 09。）

## 6.1 为什么测试是本任务的核心

#68262 的风险是 **miscompile**——错误的可见性导致 LLVM 删/静态化了实际仍被调用的函数。这种 bug：

- **不会在编译期报错**，只在运行时表现为「调用到错误目标 / 崩溃 / 结果错」；
- **依赖跨 crate + LTO + 运行**才能暴露，单文件编译看不出来。

所以测试必须能**运行**跨 crate、fat LTO 的产物并断言行为。这决定了 run-make 是本任务的主力。

## 6.2 测试金字塔（本任务版）

```
        ┌───────────────────────────┐
        │  run-make（跨 crate+LTO+运行）│  ← 正确性/miscompile 守护，主力
        ├───────────────────────────┤
        │ assembly-llvm（真去虚化断言）  │  ← WPD 可观测（阶段四）
        ├───────────────────────────┤
        │ codegen-llvm（IR 元数据/数值） │  ← 元数据形态 + vcall_visibility 值
        ├───────────────────────────┤
        │  ui（build-pass / 诊断）       │  ← flag 组合能编过、诊断正确
        └───────────────────────────┘
```

## 6.3 codegen-llvm 测试：断言元数据与数值

**测什么**：
- vtable global 上有 `!type` + `!vcall_visibility`；
- 具体 `!vcall_visibility` 数值（0/1/2）符合保守语义；
- 调用点是 `llvm.type.checked.load`（offset、typeid 正确）；
- 未被动态调用的函数**不出现**（VFE 生效）。

**怎么写**：复制 `tests/codegen-llvm/virtual-function-elimination.rs` 的结构（教程 09.3 已逐行拆解），改 trait 可见性/包装/CGU 条件，更新 CHECK 断言。注意：
- 固定四件套 flag：`-Zvirtual-function-elimination -Clto -Copt-level=3 -Csymbol-mangling-version=v0`；
- offset 依赖指针宽度 → 64 位与 32 位分文件，加 `//@ ignore-32bit` / 单独 `-32bit.rs`。

**阶段二注意**：保守化后，现有测试里私有 `T`→`!{i64 2}` 的断言**可能要改**成更保守的值。改时加注释说明「为什么从 2 降级」，把语义变化记录在测试里。

## 6.4 run-make 测试：跨 crate 正确性（重中之重）

**测什么**：私有 trait 经 public wrapper / `#[inline]` / 泛型逃逸到下游 crate 后，fat LTO 不会因过度删除/去虚化而 miscompile。

**结构**（北极星例，对应 T2.1）：

```
tests/run-make/vfe-private-trait-escape/
├── rmake.rs          ← 驱动：编 upstream → 编 downstream → fat LTO 链接 → 运行 → 断言
├── upstream.rs       ← 私有 trait Foo + pub struct FooBox + pub make_foo + #[inline] pub f
└── downstream.rs     ← 调用 f(make_foo())，其结果依赖 Foo::foo 未被误删
```

**关键点**：
- 让 `Foo::foo` 的行为**可观测**（比如返回一个特定值，或有副作用），这样「被误删/误去虚化」时运行结果会错或崩，测试才能捕获。
- 用 fat LTO（`-Clto=fat`）+ `-Zvirtual-function-elimination` 链接。
- `rmake.rs` 里用 rust-make 提供的辅助 API 编译各 crate、跑产物、断言退出码/输出。参考 `tests/run-make/` 下已有例子和 `src/doc/rustc-dev-guide/src/tests/compiletest.md`。

**为什么必须 run-make**：codegen 测试是单 crate 的，无法表达「下游 crate 内联上游 `#[inline]` 函数后触发调用」这一跨 crate 语义——而这正是 miscompile 的触发条件。

## 6.5 assembly-llvm 测试：证明真去虚化（阶段四）

**测什么**：在安全场景（唯一可行 impl + fat LTO + 安全可见性）下，间接虚调用**真的**变成直接调用/内联。

**怎么写**：`tests/assembly-llvm/`，断言最终汇编里是 `call <具体符号>` 或内联后的效果，而非通过寄存器的间接 `call`。同时建一个**负向对照**：保守场景下应保留间接调用。

## 6.6 ui 测试：flag 组合与诊断

**测什么**：
- `-Zvirtual-function-elimination` + fat LTO 能 build-pass（现有 `tests/ui/codegen/virtual-function-elimination.rs`）；
- 若你新增/修改诊断（如某些不支持场景给出错误/警告），用 ui 测试断言 stderr（`--bless` 更新快照）。

## 6.7 「有区分度」纪律（务必执行）

每个新测试都要证明它真的测到了东西（教程 09.7）：

1. **暴露型测试**（如 T2.1 北极星例）：在**修复前**的实现上跑，必须**失败/复现 miscompile**；修复后转绿。若修复前就绿，说明没测到，重写。
2. **锁定型测试**（回归守护）：改动前后都绿，但**故意回退修复**时应变红。
3. **数值型断言**（vcall_visibility 值）：确认断言的数字确实是当前实现产出的（用 `--emit=llvm-ir` 核对），不要照抄旧值。

> 实操建议：对暴露型测试，先 `git stash` 你的修复，跑测试看它红，再 `git stash pop`，跑测试看它绿。把这个验证过程写进 PR 描述。

## 6.8 覆盖矩阵（建议至少覆盖）

| 场景 | trait 可见性 | 逃逸路径 | CGU | 期望（保守） |
| --- | --- | --- | --- | --- |
| 纯本地私有 | private | 无 | 1 | 可较激进（若证明安全） |
| 私有经 pub wrapper | private | `pub struct(Box<dyn>)` + `pub fn` | 任意 | 保守（不得 TU/过窄） |
| 私有经 `#[inline]` | private | `#[inline] pub fn` | 任意 | 保守 |
| 私有经泛型 | private | `pub fn g<T>(&dyn Trait)` | 任意 | 保守 |
| blanket impl | 任意 | `impl<T> Trait for T` | 任意 | 保守（候选不封闭） |
| 公开 trait | public | — | 任意 | LinkageUnit（LTO 下） |
| 多 CGU | private | — | 16 | 不得随意 TU |
| 唯一 impl 安全场景 | — | — | — | 允许真去虚化（阶段四正向） |

## 6.9 本篇小结

- 主力是 **run-make**（跨 crate + fat LTO + 运行），因为 miscompile 只在这种条件下暴露。
- codegen-llvm 测数值/元数据形态（保守化后可能要改期望值）；assembly 测真去虚化；ui 测 build-pass/诊断。
- 每个测试必须验证「有区分度」：暴露型改前应红，锁定型回退应红。
- 用 6.8 的矩阵作为覆盖清单。

下一篇：[`07-risks-and-open-questions.md`](07-risks-and-open-questions.md)
