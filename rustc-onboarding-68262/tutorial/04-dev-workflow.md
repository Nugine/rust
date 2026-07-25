# 04 · 开发工作流

目标：把「构建 / 测试 / 调试」组织成一个你每天都能重复的高效闭环。上一篇讲了单个命令，这一篇讲怎么把它们串成节奏，以及针对 #68262 这种「codegen + 元数据」任务的具体打法。

## 4.1 核心闭环：改 → check → 定向 test

编译器开发的黄金法则是**缩短反馈环**。推荐这个由快到慢的三档：

1. **秒~分钟级：`./x.py check`**
   改完代码先 `check`，只做类型检查，不生成产物。绝大多数编译错误（借用、类型、拼写）在这一步就暴露。**养成每次改完先 check 的习惯。**

2. **分钟级：`./x.py build`（stage1）+ 单文件 test**
   check 过了、想看真实行为，再构建 stage1 并只跑**一个**相关测试：
   ```bash
   ./x.py test tests/codegen-llvm/virtual-function-elimination.rs
   ```

3. **较慢：跑一整个套件**
   只有在准备提 PR、要确认没有回归时，才跑更大范围（见 4.5）。

> 反模式：改一行就 `./x.py test tests/ui`（全量 UI 测试）。那会等很久且信息噪声大。**永远先定向到最相关的少数测试。**

## 4.2 针对 #68262 的专用闭环

本任务的观察对象是「LLVM IR 里 vtable 上的元数据 + 虚调用点」。所以最有用的闭环其实是「**改 rustc → 用它编译一个小 case → 看 `.ll`**」，比跑测试更直接：

```bash
# 0. 准备一个最小复现文件 /tmp/vfe/demo.rs（放 /tmp，别污染仓库）
mkdir -p /tmp/vfe

# 1. 改 rustc 代码后构建 stage1
./x.py build

# 2. 用新 rustc 直接产出 LLVM IR
build/host/stage1/bin/rustc --edition 2021 \
    -Zvirtual-function-elimination -Clto=fat -Copt-level=3 \
    -Csymbol-mangling-version=v0 \
    --emit=llvm-ir -o /tmp/vfe/demo.ll /tmp/vfe/demo.rs

# 3. 看 vtable 上的 !type / !vcall_visibility，以及调用点的 llvm.type.checked.load
grep -n "vcall_visibility\|!type\|type.checked.load" /tmp/vfe/demo.ll
```

（`build/host/` 里的 `host` 实际是你的 target triple 目录名，用 `ls build/` 确认。）

这个闭环让你**肉眼确认**「改动有没有让元数据按预期变化」，是设计阶段最快的验证手段。把它变成一个小脚本（放 `/tmp`）反复用。

## 4.3 增量与缓存

- **rustc 自身增量**：`bootstrap.toml` 里开 `rust.incremental = true`，让编译 rustc 本身走增量，改小改动重建更快。
- **只 check 不 build**：设计/探索阶段能 check 就别 build。
- **别频繁切 stage**：stage2 会几乎重头再来一遍，日常别碰。
- **善用 `--keep-stage`**：如果你只改了不影响 std 的编译器代码，`./x.py build --keep-stage 1` 可以跳过一些重建（有风险，改动跨层时别用）。这属于进阶技巧，先掌握基本闭环再说。

## 4.4 读代码的工作流

编译器代码巨大，靠「读」不如靠「查 + 顺藤摸瓜」：

1. **从一个已知锚点出发**：本任务的锚点就是调查报告里那几个符号，比如 `apply_vcall_visibility_metadata`。
2. **grep 符号**：
   ```bash
   grep -rn "apply_vcall_visibility_metadata" compiler/
   ```
3. **看调用者和被调用者**：一个函数「谁调它」「它调谁」往往比函数体本身更能说明它在流水线里的位置。
4. **顺着 query 走**：看到 `tcx.xxx(...)` 就知道这是一个 query，可以再去搜 `xxx` 的定义（教程 06 讲 query 定义在哪）。
5. **别怕 `rustc_middle`**：大多数类型/query 定义都汇集在这里。

## 4.5 提交前的检查清单

准备提 PR 时，本地至少过一遍（对应教程 09、11）：

- `./x.py fmt`（格式化，否则 CI 的 tidy 会红）
- `./x.py check`（整体编过）
- 跑与改动**相关**的测试套件，例如：
  ```bash
  ./x.py test tests/codegen-llvm/virtual-function-elimination.rs
  ./x.py test tests/ui/codegen/virtual-function-elimination.rs
  ```
- 如果加了新测试，确认新测试**确实会因为你的改动而通过/失败**（先 revert 改动看它失败，能防止「测了个寂寞」）。
- `./x.py test tidy`（代码规范/许可证头等静态检查）——CI 一定会跑，本地先过省来回。

## 4.6 心态与节奏

- **一次只推进一小步**：改一个点、验证一个点。编译器里「看起来无害的改动」经常有远处的副作用。
- **先写会失败的测试，再写实现**：本任务尤其如此——很多价值就在「把已知的 miscompile 风险固化成测试」。
- **保守优先**：涉及可见性/优化时，「不优化」永远比「错误优化」安全。拿不准就回退到 `Public`（见 roadmap）。
- **记录你的观察**：每个 case 的 `.ll` 结果、每个疑问，随手记在 `/tmp` 的笔记里，别靠脑子记行号。

## 4.7 本篇小结

- 反馈环由快到慢：`check` → `build`+单文件 `test` → 大套件。
- 本任务最有用的闭环是 `build` + `--emit=llvm-ir` + grep 元数据，直接肉眼验证。
- 读代码 = 从已知符号 grep + 追调用关系 + 顺 query。
- 提 PR 前：fmt / check / 相关测试 / tidy，并验证新测试真的有区分度。

## 延伸阅读

- rustc-dev-guide「Suggested workflows for faster iteration」：`src/doc/rustc-dev-guide/src/building/suggested.md`
- 在线：<https://rustc-dev-guide.rust-lang.org/building/suggested.html>

下一篇：[`05-compiler-pipeline.md`](05-compiler-pipeline.md) —— 现在有了工具，来看编译器内部从源码到 LLVM IR 到底经过哪些阶段。
