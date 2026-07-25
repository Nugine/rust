# 03 · 构建系统（x.py / bootstrap）

目标：让你能在本机把 rustc 构建出来、并理解构建为什么慢、怎么只构建你需要的部分。构建 rustc 和构建普通 Rust 项目**很不一样**，因为「用来编译编译器的编译器」本身也在这个仓库里——这就是 bootstrap（自举）。

> 现实预期：**第一次完整构建 rustc 通常要几十分钟到一两个小时**，磁盘占用可数十 GB。所以本篇的重点之一是「怎么少构建」。本任务大量工作是写测试和读代码，很多时候你甚至不需要完整构建。

## 3.1 入口：`x.py` / `x` / `x.ps1`

仓库根有三个等价入口，选一个：

- `./x.py <子命令> ...`（需要 python3）
- `./x <子命令> ...`（shell 包装）
- `.\x.ps1 <子命令> ...`（Windows PowerShell）

它们都只是薄壳，真正的构建逻辑在 `src/bootstrap/`（用 Rust 写的构建编排器，叫 **bootstrap**）。

常用子命令：

| 命令 | 作用 |
| --- | --- |
| `./x.py setup` | 交互式生成配置文件 `bootstrap.toml`（首次使用先跑它） |
| `./x.py check` | 只做类型检查（`cargo check` 级别），**最快的「代码能不能编过」反馈** |
| `./x.py build` | 真正构建（产出可用的 rustc/std） |
| `./x.py test <路径或套件>` | 构建并跑测试 |
| `./x.py fmt` | 格式化 |
| `./x.py clippy` | 跑 clippy |
| `./x.py doc` | 构建文档 |

## 3.2 stage 概念（bootstrap 的核心）

这是最容易懵的点。构建 rustc 分「阶段（stage）」：

- **stage0**：一个**已经预编译好、从官方下载**的 rustc（beta 编译器）。它是「种子」，用来编译我们仓库里的源码。
- **stage1**：用 stage0 编译**我们仓库里的 rustc 源码**，得到的编译器。它已经包含你的改动，但它是「被旧编译器编译出来的」。
- **stage2**：用 stage1 再编译一次 rustc 源码，得到「用新编译器编译新编译器」的产物——这才是发布级别、完全自举的编译器。

对本任务的实用结论：

- **改了 rustc 代码、想验证行为**：`./x.py build` 默认产出 **stage1** 编译器，通常足够测试你的改动。
- **多数测试**：`./x.py test tests/codegen-llvm/...` 会自动构建所需 stage 的编译器再跑测试。
- 只有做「最终验收 / 性能 / 发布」才需要 stage2。**日常迭代尽量停在 stage1。**

## 3.3 配置文件 `bootstrap.toml`

复制模板并按需修改：

```
cp bootstrap.example.toml bootstrap.toml
```

（或者 `./x.py setup` 帮你生成。）几个对开发者最重要的键（模板里都有注释，行号见 `bootstrap.example.toml`）：

- **`profile`**（`bootstrap.example.toml:28` 附近）：选一个预设。开发编译器选 `compiler`（或 `codegen` 若你要动 LLVM 绑定）。`./x.py setup` 会问你选哪个。
- **`llvm.download-ci-llvm`**（`:75` 附近，默认 `true`）：**极其重要**。设为 `true` 时，bootstrap **不在本机从源码编译 LLVM**，而是下载 CI 预构建好的 LLVM。这能把首次构建从「几小时」降到「几十分钟」。
  - **但注意**：本任务（#68262）**会不会改到 LLVM 侧？** 需求主要是让 rustc 生成正确的元数据，LLVM 的去虚化 pass 已存在，所以**通常不需要从源码构建 LLVM**，`download-ci-llvm = true` 就够。只有当你要改 `rustc_codegen_llvm` 里跨 FFI 到 LLVM C++ 的新 API，且该 API 在 CI 版本里没有时，才需要本地 LLVM 源码构建。
- **`rust.incremental`**（`:722` 附近）：开启后 rustc 自身的构建走增量，迭代更快。
- **`rust.debug` / assertions**：开发期通常开 debug assertions，能更早暴露内部不变量被破坏。

> 建议起步配置：`./x.py setup` 选 `compiler` profile，保留 `download-ci-llvm = true`，开 `rust.incremental = true`。

## 3.4 最短「能跑起来」路径

假设你只想验证「我的 rustc 改动有没有编译错误 + 一个 codegen 测试过不过」：

```bash
# 1. 一次性配置（选 compiler profile）
./x.py setup

# 2. 快速类型检查（秒级~分钟级，先确保编过）
./x.py check

# 3. 构建 stage1 编译器（首次较久，之后增量快）
./x.py build

# 4. 跑与本任务相关的 codegen 测试
./x.py test tests/codegen-llvm/virtual-function-elimination.rs
```

## 3.5 只构建/测试你需要的部分

完整构建很贵，学会「缩小范围」是提高效率的关键：

- **只 check 某个 crate**：`./x.py check --stage 1`（check 通常最快）。
- **只跑单个测试文件**：把测试文件路径直接给 `test`，如上面第 4 步。compiletest 会只挑这个文件。
- **按名字过滤测试**：`./x.py test tests/ui --test-args virtual-function`（`--test-args` 传给测试驱动器做子串过滤）。
- **跳过重复构建**：同一 stage 的增量构建会复用缓存，不必每次从头。

> 关于本任务的现实建议：**很多设计工作（读代码、理解可见性语义、设计 query）根本不需要构建**。真正需要构建的时机是：你已经改了元数据生成逻辑、要用 codegen 测试观察 LLVM IR 变化时。所以别被「首次构建很久」吓退——先读、先设计、先写测试骨架。

## 3.6 用刚构建的 rustc 手动编译一个文件

调试单个 case 时很有用（教程 10 会深入）。bootstrap 提供 `rustc` 直通：

```bash
# 用你构建出的 stage1 rustc 编译某个文件，并观察它的行为
./x.py run  # 一般不用；更常用的是下面这种
build/host/stage1/bin/rustc --edition 2021 foo.rs -Zvirtual-function-elimination -Clto=fat --emit=llvm-ir
```

`build/` 目录是 bootstrap 的输出目录，`build/<host-triple>/stage1/bin/rustc` 就是你构建出来、带你改动的编译器。`--emit=llvm-ir` 会输出 `.ll` 文件，让你直接看 vtable 上的 `!type` / `!vcall_visibility`。（准确的 host triple 目录名可以 `ls build/` 查看。）

## 3.7 常见坑

- **忘了 `bootstrap.toml`**：不配置也能跑，但会用默认（可能从源码编 LLVM，很慢）。先 `./x.py setup`。
- **改了 codegen 代码却没重新 `build`**：`test` 通常会自动重建，但如果你手动用 `build/.../rustc`，记得先 `./x.py build`。
- **stage 混淆**：报错/行为和预期不符时，确认你测的是 stage1 且已包含最新改动。
- **磁盘不足**：`build/` 会很大；LLVM 源码构建更大。优先 `download-ci-llvm`。
- **子模块未初始化**：`src/llvm-project`、`src/doc/rustc-dev-guide` 等是 git 子模块，bootstrap 通常会自动处理；若报缺失，按提示更新子模块。

## 3.8 本篇小结

- 构建入口是 `./x.py`，逻辑在 `src/bootstrap/`。
- stage0（下载的种子）→ stage1（含你的改动，日常够用）→ stage2（完全自举，验收才需要）。
- 配置文件 `bootstrap.toml`：选 `compiler` profile、保留 `download-ci-llvm=true`、开增量。
- 日常闭环：`check`（快反馈）→ `build`（stage1）→ `test <单个文件>`。
- 本任务多数设计工作**不需要构建**；需要时用 `--emit=llvm-ir` 直接看 vtable 元数据。

## 延伸阅读

- rustc-dev-guide「How to build and run the compiler」：`src/doc/rustc-dev-guide/src/building/how-to-build-and-run.md`
- 「Quickstart」：`src/doc/rustc-dev-guide/src/building/quickstart.md`
- 「Suggested workflows」：`src/doc/rustc-dev-guide/src/building/suggested.md`
- 在线：<https://rustc-dev-guide.rust-lang.org/building/how-to-build-and-run.html>

下一篇：[`04-dev-workflow.md`](04-dev-workflow.md) —— 把构建、测试、调试组织成一个高效的日常闭环。
