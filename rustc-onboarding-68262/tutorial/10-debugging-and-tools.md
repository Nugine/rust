# 10 · 调试与工具

目标：给你一套「出问题时怎么查」的工具箱——打日志、看各阶段中间表示、导出 LLVM IR、用 `-Z` 开关观察内部、定位 ICE（编译器内部崩溃）。这些是本任务实操时的日常。

## 10.1 观察 LLVM IR（本任务最常用）

本任务的核心观察对象就是 LLVM IR 里的 vtable 元数据和调用点。用你构建的 rustc 直接产出 `.ll`：

```bash
build/host/stage1/bin/rustc --edition 2021 \
    -Zvirtual-function-elimination -Clto=fat -Copt-level=3 -Csymbol-mangling-version=v0 \
    --emit=llvm-ir -o /tmp/vfe/demo.ll /tmp/vfe/demo.rs

grep -n "vcall_visibility\|!type\|type.checked.load\|@vtable" /tmp/vfe/demo.ll
```

要点：
- `--emit=llvm-ir` 产出的是**LTO 前、优化后**的单 crate IR。要看**去虚化是否真的发生**（LTO 之后），单 crate `--emit=llvm-ir` 不够——去虚化在链接期 LTO 才跑。观察最终效果得用 run-make 或 `-Csave-temps` / 链接期产物（见 10.6）。
- 加 `-Copt-level=0` 可以看「未优化」的原始形态，对照理解 rustc 到底发了什么。

## 10.2 打日志：`tracing` / `RUSTC_LOG`

rustc 用 `tracing` 做结构化日志，很多函数标了 `#[instrument]`（比如 `get_vtable` 上就有 `#[instrument(level="debug", skip(cx))]`）。用环境变量开：

```bash
# 只看某个模块的 debug 日志
RUSTC_LOG=rustc_codegen_ssa::meth=debug build/host/stage1/bin/rustc ... 2> log.txt
# 看整个 codegen_llvm
RUSTC_LOG=rustc_codegen_llvm=debug build/host/stage1/bin/rustc ...
```

- 语法是 `RUSTC_LOG=模块路径=级别`，可逗号分隔多个。
- 想临时加日志：在关心的函数里插 `debug!(...)` / `trace!(...)`（rustc 内部宏），重新 `./x.py build` 即可。**这是理解「某个 case 走了哪条分支」最直接的手段**——比如在 `apply_vcall_visibility_metadata` 里打印算出来的 `vcall_visibility` 和 `trait_vis`。

> 注意：debug/trace 日志只有在**开了 debug assertions / debug logging** 的构建里才全量可见。开发构建（`compiler` profile）通常已启用。

## 10.3 看 MIR

虽然本任务主要在 codegen 层，但有时要确认「虚调用在 MIR 里长什么样」：

```bash
build/host/stage1/bin/rustc --edition 2021 --emit=mir -o /tmp/vfe/demo.mir /tmp/vfe/demo.rs
# 或者用 -Zdump-mir 把各阶段 MIR dump 到 mir_dump/ 目录
build/host/stage1/bin/rustc -Zdump-mir=all /tmp/vfe/demo.rs
```

在 MIR 里你能看到虚调用被表示成「从 vtable 取指针 + 间接 call」。

## 10.4 有用的 `-Z` 观察开关

| 开关 | 作用 |
| --- | --- |
| `-Zprint-type-sizes` | 打印类型布局/大小 |
| `-Zdump-mir=all` | dump 各阶段 MIR |
| `-Zverbose-internals` | 更啰嗦的内部打印（类型等） |
| `-Cllvm-args=...` | 把参数透传给 LLVM（可开 LLVM pass 的调试输出） |
| `-Csave-temps` | 保留中间产物（含 LTO 各阶段 `.bc`） |
| `-Zvirtual-function-elimination` | 本任务的主角开关 |

`-Z` 开关全集可用 `rustc -Z help` 查看，定义在 `rustc_session/src/options.rs`。

## 10.5 定位 ICE（编译器内部错误）

如果你的改动让 rustc 自己 panic（ICE, Internal Compiler Error）：

- 加 `RUST_BACKTRACE=1` 拿到 panic 的 Rust 调用栈：
  ```bash
  RUST_BACKTRACE=1 build/host/stage1/bin/rustc ... 2>&1 | less
  ```
- 栈顶通常直指你改坏的地方。`bug!(...)` / `span_bug!(...)` 是 rustc 内部的「断言失败」宏，触发它说明某个不变量被破坏。
- 开 debug assertions 的构建会更早、更清晰地暴露问题（`bootstrap.toml` 里 `rust.debug-assertions = true`）。

## 10.6 观察「去虚化是否真的发生」

这是本任务验收的关键，也是最容易被忽略的：`--emit=llvm-ir` 看到的是 rustc 发给 LLVM 的东西，**不等于**去虚化已发生。要验证 `WholeProgramDevirt` 真把间接调用变直接：

1. **run-make + 反汇编/IR dump**：在 LTO 链接后的产物上检查（`-Csave-temps` 会保留 LTO 阶段的 `.bc`，可用 `llvm-dis` 转成 `.ll`）。
2. **assembly 测试**：`tests/assembly-llvm/`，断言最终汇编里是直接 `call 目标` 而非间接 `call` 通过寄存器。
3. **行为对照**：构造「若被错误去虚化则结果不同」的运行测试。

> 调查报告特别提醒过：如果发现 WPD pass 根本没在当前 LTO pipeline 触发，就得去看 `compiler/rustc_codegen_llvm/src/back/lto.rs` 的 pass 配置，而不是继续改前端元数据。10.4 的 `-Cllvm-args` 可以帮你确认 pass 是否运行。

## 10.7 常用手动实验清单（本任务）

把这几个最小 case 各写一个文件放 `/tmp/vfe/`，反复用 10.1 的命令观察 `vcall_visibility` 数值：

1. 私有 `trait T` + 单 impl（现状标 2，问：安全吗？）
2. 公开 `pub trait V`（现状标 1）
3. 私有 trait 经 `pub struct W(Box<dyn T>)` 逃逸
4. 私有 trait 经 `#[inline] pub fn` 逃逸
5. blanket impl `impl<X> T for X`
6. `-Ccodegen-units=16`（多 CGU）vs `-Ccodegen-units=1`

对每个记录：vtable 是否有 `!type`、`!vcall_visibility` 的值、调用点是否 checked load。**这份对照表就是你设计保守化规则的原始数据。**

## 10.8 本篇小结

- 看元数据：`--emit=llvm-ir` + grep（LTO 前）；看真去虚化：run-make/assembly/`-Csave-temps`（LTO 后）。
- 打日志：`RUSTC_LOG=模块=debug`，或临时插 `debug!()` 重编——追分支最有效。
- ICE：`RUST_BACKTRACE=1` + debug assertions。
- 建一份「六个最小 case × 三项观察」的对照表，作为保守化设计的数据基础。

## 延伸阅读

- rustc-dev-guide「Debugging the compiler」：`src/doc/rustc-dev-guide/src/compiler-debugging.md`
- 「Using tracing / RUSTC_LOG」：同上一带
- 在线：<https://rustc-dev-guide.rust-lang.org/compiler-debugging.html>

下一篇：[`11-contributing.md`](11-contributing.md) —— 最后一步：怎么把补丁提上去（PR / review / bors / CI）。
