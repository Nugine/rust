# 教程总览（00 · Index）

这套教程共 11 篇，目标是把一个「会写 Rust 但没碰过编译器源码」的人，带到「能独立在 rustc 里定位、修改、验证一处 codegen 行为」的水平。整体是围绕 #68262（整程序去虚化）裁剪的，所以在讲通用知识时会不断把镜头拉回到 vtable / trait object / LLVM 元数据这条线上。

## 每篇讲什么

| 篇号 | 标题 | 你会得到什么 | 与 #68262 的关系 |
| --- | --- | --- | --- |
| 01 | 定位与心智模型 | rustc 是什么、它和 LLVM 的分工、一个「查询驱动」的编译器长什么样 | 打底 |
| 02 | 仓库结构与 crate 划分 | `compiler/`、`library/`、`tests/`、`src/` 各是什么，`rustc_*` 那 70 多个 crate 怎么分层 | 知道去哪个 crate 改代码 |
| 03 | 构建系统（x.py / bootstrap） | 怎么配置、怎么构建 rustc、stage0/1/2 是什么、怎么只构建你要的部分 | 能把改动跑起来 |
| 04 | 开发工作流 | 「改一行 → 构建 → 测试」的最短闭环、增量技巧、常见坑 | 决定你迭代快不快 |
| 05 | 编译流水线 | 从源码到机器码要经过哪些阶段（lex/parse/HIR/类型检查/MIR/单态化/codegen） | 定位任务在流水线的哪一段 |
| 06 | 核心数据结构 | `TyCtxt`、query 系统、`DefId`、`Ty`、HIR/MIR 的关系 | 读懂 codegen 代码的前提 |
| 07 | trait 与 vtable | `dyn Trait` 怎么表示、vtable 布局、unsizing coercion、可见性 | **本任务的核心概念** |
| 08 | codegen 与 LLVM 交互 | `rustc_codegen_ssa` 与 `rustc_codegen_llvm` 的分层、intrinsic、给 global 挂 metadata | **本任务改动的落点** |
| 09 | 测试基础设施 | compiletest、codegen 测试（FileCheck）、ui 测试、run-make | **本任务大量工作是写测试** |
| 10 | 调试与工具 | 打日志、看 MIR、导出 LLVM IR、`-Z` 开关、定位 ICE | 出问题时怎么排查 |
| 11 | 贡献流程 | PR、review、bors、r+、CI 的运作方式 | 最终怎么把补丁提上去 |

## 怎么用这张表

- **只想快速能动手**：01 → 03 → 04 → 09，再直接跳到 roadmap。
- **想真正理解任务**：按顺序全读，重点 05/06/07/08。
- **卡在某个概念**：每篇末尾有「延伸阅读」，指向 rustc-dev-guide 的对应章节。

## 一个贯穿全教程的例子

我们会反复使用这段代码作为「跟踪对象」：

```rust
trait Animal {
    fn noise(&self) -> u32;
}

struct Cat;
impl Animal for Cat {
    fn noise(&self) -> u32 { 1 }
}

fn make(a: &dyn Animal) -> u32 {
    a.noise() // ← 这是一次「虚调用」(virtual call)，本任务就是想在合适条件下把它变成直接调用
}
```

后面每讲到一个阶段，就会问：「上面这段代码在这个阶段变成了什么？」——从 token，到 HIR，到 `Ty`，到 MIR，到 vtable，到 LLVM 里那条 `llvm.type.checked.load`。

> 提示：本教程给出的所有命令都假设你的当前目录是仓库根 `/home/runner/work/rust/rust`（或你本机对应的 rust 克隆根目录）。

下一篇：[`01-orientation.md`](01-orientation.md)
