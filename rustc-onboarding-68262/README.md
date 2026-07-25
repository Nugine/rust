# rustc 上手 + issue #68262 实施手册

本目录是为「**接手 rust-lang/rust#68262（whole program devirtualization，整程序去虚化）但此前没有 rustc 开发经验**」的贡献者准备的一整套引导材料。

它由两部分组成：

1. **`tutorial/`** —— 一套从零开始的 rustc 编译器开发教程。它不假设你读过编译器源码，只假设你会写 Rust。读完之后你应该能自己构建 rustc、跑测试、看懂编译流水线，并知道去哪里改代码。
2. **`roadmap/`** —— 针对 #68262 这个具体任务的实施路线。它建立在 [`../issue-68262-investigation.md`](../issue-68262-investigation.md)（前置调查报告）之上，把「要做什么、按什么顺序做、怎么验证」拆成可执行的阶段和任务。

> 术语约定：本手册里 **WPD** = whole-program devirtualization（整程序去虚化），**VFE** = virtual function elimination（虚函数消除）。这两者相关但不相同，区别见 roadmap/02 和 roadmap/08。

## 建议阅读顺序

如果你是第一次接触 rustc：

1. 先通读 `tutorial/00-index.md`，它会告诉你每篇讲什么。
2. 按 `tutorial/01` → `tutorial/11` 顺序读教程。**其中 07（trait 与 vtable）和 08（codegen 与 LLVM）与本任务最相关**，可以多花时间。
3. 然后读 `roadmap/00-index.md`，再按顺序读 roadmap。
4. 想动手时，回到 `roadmap/05-detailed-tasks.md`，从「阶段一」开始。

如果你已经熟悉 rustc，只想接任务：

1. 直接读 `../issue-68262-investigation.md`（结论摘要 + 代码地图）。
2. 再读 `roadmap/03` → `roadmap/07`。
3. 教程里挑 07/08 复习即可。

## 这套文档的定位与边界

- 这套文档是**工作笔记/交接材料**，不是要合入 rust-lang/rust 上游的正式文档。放在仓库根下的独立目录里，避免触发上游 `tidy`/文档检查。
- 教程里引用的**行号和文件路径都以本仓库当前状态为准**，并已逐条核对（截至 rustc `1.99.0`，见仓库根 `src/version`）。上游演进很快，行号会漂移；**遇到对不上时，用文中给出的符号名（函数名、常量名）去 grep，而不要死记行号**。
- 官方权威资料是 **rustc-dev-guide**（本仓库内就有一份：`src/doc/rustc-dev-guide/`，在线版 <https://rustc-dev-guide.rust-lang.org/>）。本教程与它互补：dev-guide 覆盖面广而全，本教程是围绕本任务裁剪过、带具体代码坐标的「快速通道」。

## 目录

```
rustc-onboarding-68262/
├── README.md              ← 你在这里
├── tutorial/              ← 通用 rustc 开发教程
│   ├── 00-index.md
│   ├── 01-orientation.md
│   ├── 02-repo-layout.md
│   ├── 03-build-system.md
│   ├── 04-dev-workflow.md
│   ├── 05-compiler-pipeline.md
│   ├── 06-core-datastructures.md
│   ├── 07-traits-and-vtables.md
│   ├── 08-codegen-and-llvm.md
│   ├── 09-testing.md
│   ├── 10-debugging-and-tools.md
│   └── 11-contributing.md
└── roadmap/               ← #68262 实施路线
    ├── 00-index.md
    ├── 01-problem-statement.md
    ├── 02-llvm-background.md
    ├── 03-current-state-map.md
    ├── 04-phased-plan.md
    ├── 05-detailed-tasks.md
    ├── 06-testing-strategy.md
    ├── 07-risks-and-open-questions.md
    └── 08-glossary.md
```

## 进度约定

每篇文档尽量做到自包含，但会互相引用（用相对路径）。如果你在读的过程中发现文中代码坐标已漂移，欢迎顺手更新；更新时请保留「符号名 + 大致位置」的写法。
