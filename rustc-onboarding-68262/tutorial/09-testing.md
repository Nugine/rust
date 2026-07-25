# 09 · 测试基础设施

目标：讲清 rustc 的测试体系——compiletest、测试指令（directives）、codegen 测试的 FileCheck 断言、ui/run-make 的用法。本任务**大量工作量是写测试**，所以这一篇要能让你照着现有测试改出新测试。

## 9.1 compiletest：测试驱动器

`tests/` 下的测试不是普通 `#[test]`，而是由 **compiletest**（`src/tools/compiletest`）驱动的：它读每个测试文件顶部的**指令注释**，用被测 rustc 按指令编译，再按测试类型检查结果（stderr 匹配、IR 匹配、运行退出码等）。

运行方式（教程 03/04 讲过）：

```bash
./x.py test tests/codegen-llvm/virtual-function-elimination.rs   # 单文件
./x.py test tests/ui --test-args virtual-function                # 子串过滤
```

## 9.2 测试指令（directives）

测试文件顶部以 `//@` 开头的行是指令。常见的：

| 指令 | 作用 |
| --- | --- |
| `//@ compile-flags: ...` | 传给 rustc 的额外命令行参数 |
| `//@ ignore-32bit` / `//@ only-x86_64` | 平台过滤（offset 依赖指针宽度时常用） |
| `//@ revisions: a b` | 一个文件跑多个配置变体 |
| `//@ needs-llvm-components: ...` | 声明依赖的 LLVM 组件 |
| `//@ aux-build: foo.rs` | 先把 `foo.rs` 编成辅助 crate（**跨 crate 测试关键**） |
| `//@ build-pass` / `//@ check-pass` / `//@ run-pass` | 期望：能构建 / 能检查 / 能运行通过 |

指令的完整清单见 `src/doc/rustc-dev-guide/src/tests/directives.md`。

## 9.3 codegen 测试与 FileCheck（本任务主力）

`tests/codegen-llvm/` 里的测试：compiletest 让 rustc **产出 LLVM IR**，再用 **FileCheck**（LLVM 自带的工具）按文件里的 `// CHECK:` 注释断言 IR 形态。

以本任务现有的 `tests/codegen-llvm/virtual-function-elimination.rs` 为例（**逐行已核对**），拆解它教了我们什么：

```rust
//@ compile-flags: -Zvirtual-function-elimination -Clto -Copt-level=3 -Csymbol-mangling-version=v0
//@ ignore-32bit
```
- 编译参数固定了本任务的四件套：开 VFE、fat LTO、O3、v0 mangling。
- `ignore-32bit`：因为断言里写死了 64 位的字节 offset（下面会看到 `i32 24` 等），32 位另有一个 `-32bit.rs` 文件。

```
// CHECK: @vtable.0 = {{.*}}, !type ![[TYPE0:[0-9]+]], !vcall_visibility ![[VCALL_VIS0:[0-9]+]]
```
- 断言：名为 `@vtable.0` 的 global 上，同时挂了 `!type` 和 `!vcall_visibility`。
- `![[TYPE0:[0-9]+]]` 是 **FileCheck 捕获变量**：把这条 `!type` 指向的元数据编号记住，后面再用 `![[TYPE0]]` 引用，保证前后一致。

```rust
trait T { fn used(&self)->i32{1} fn unused(&self)->i32{2} ... }
// CHECK-LABEL: ; <...S as ...T>::used
// CHECK-LABEL-NOT: {{.*}}::unused
```
- `CHECK-LABEL` 断言最终 IR 里**出现** `used` 函数；`CHECK-LABEL-NOT` 断言 `unused` **不出现**——因为它从没被虚调用，VFE/GlobalDCE 应该把它删掉。**这正是「VFE 删未用函数」的可观测证据**。

```rust
fn taking_t(t: &dyn T) -> i32 {
    // CHECK: @llvm.type.checked.load({{.*}}, i32 24, metadata !"[[MANGLED_TYPE0:...]]")
    t.used()
}
```
- 断言：虚调用 `t.used()` 编译成 `llvm.type.checked.load`，offset 是 `24`（= 3 个指针 × 8 字节，跳过 drop/size/align，指向第一个方法），typeid 是捕获的 mangled 名。

```rust
// CHECK: ![[VCALL_VIS0]] = !{i64 2}   // 私有 trait T → TranslationUnit(2)
// CHECK: ![[VCALL_VIS2]] = !{i64 1}   // 公开 trait V → LinkageUnit(1)
```
- **这两行是本任务最敏感的断言**：私有 trait `T` 拿到 `vcall_visibility = 2`（TranslationUnit，最激进），公开 trait `V` 拿到 `1`（LinkageUnit）。#68262 的争议正在于：**私有 trait 真的能安全地标 2 吗？** 如果它通过 wrapper/inline 逃逸，标 2 就是 miscompile。你未来的保守化改动，很可能**需要修改这里期望的数值**（把某些从 2 降到 1 或 0），并**新增覆盖逃逸场景的测试**。

> 小结：这个文件同时演示了三类断言——(a) vtable 上有元数据，(b) 调用点是 checked load，(c) 具体 vcall_visibility 数值 + 未用函数被删。你写新测试基本就是复制这套结构再改条件。

## 9.4 FileCheck 常用语法速查

| 写法 | 含义 |
| --- | --- |
| `// CHECK: foo` | IR 中必须按顺序出现匹配 `foo` 的行 |
| `// CHECK-NEXT:` | 紧接上一条的下一行 |
| `// CHECK-NOT: foo` | 两条 CHECK 之间不得出现 `foo` |
| `// CHECK-LABEL:` | 锚点，重置匹配范围（常用于函数边界） |
| `{{正则}}` | 内嵌正则 |
| `[[NAME:正则]]` / `[[NAME]]` | 定义捕获 / 引用捕获 |

## 9.5 ui 测试

`tests/ui/` 是最大宗的测试：编译一个文件，把**实际 stderr** 和旁边的 `.stderr` 快照比对（或用 `//@ build-pass` 只要求编过）。本任务的 `tests/ui/codegen/virtual-function-elimination.rs` 属于「build-pass 回归」——确保开了这些 flag 后编译不崩、不报错。

更新快照：跑测试时加 `--bless` 会用当前输出覆盖 `.stderr`（确认输出正确后才 bless）。

## 9.6 run-make 测试（跨 crate / LTO 场景关键）

`tests/run-make/` 用一个 `rmake.rs`（Rust 驱动脚本）来做**多步骤、多 crate、真链接**的端到端测试。**本任务的很多关键安全场景需要它**，因为「私有 trait 通过下游 crate 调用导致 miscompile」这种事，单文件 codegen 测试表达不了——你需要：

1. 编一个上游 crate（含私有 trait + public wrapper + `#[inline]`）；
2. 编一个下游 crate 去调用它；
3. 开 fat LTO 链接；
4. **运行**产物，断言结果正确（如果被错误去虚化/删函数，运行结果会错或链接失败）。

这类测试是 roadmap/06 的重点。写法参考 `tests/run-make/` 下已有例子和 `src/doc/rustc-dev-guide/src/tests/compiletest.md`。

## 9.7 «先让测试失败» 的纪律

写完新测试后，务必验证它**有区分度**：

1. 先在**不改 rustc** 的情况下跑新测试——如果它已经 pass，说明它没测到你关心的行为（无效测试）。
2. 对「暴露 bug」的测试，应当在**当前（有 bug 的）实现下失败**，在你的修复后通过。
3. 对「锁定正确行为」的回归测试，改动前后都应通过，但如果有人未来回退修复它会失败。

## 9.8 汇总：本任务会用到的测试类型

| 测试类型 | 目录 | 本任务用途 |
| --- | --- | --- |
| codegen (FileCheck) | `tests/codegen-llvm/` | 断言 vtable 元数据、vcall_visibility 数值、checked load、未用函数被删 |
| ui (build-pass) | `tests/ui/codegen/` | 确保 flag 组合能编过、诊断正确 |
| run-make | `tests/run-make/` | **跨 crate + fat LTO + 运行**，验证保守性/无 miscompile |
| assembly | `tests/assembly-llvm/` | 需要断言最终汇编里去虚化真的发生时 |

## 9.9 本篇小结

- compiletest 按 `//@` 指令驱动测试；codegen 测试用 FileCheck 断言 LLVM IR。
- 现有 `virtual-function-elimination.rs` 已示范：元数据存在 + checked load + vcall_visibility 数值 + 未用函数删除。**本任务保守化后，很可能要改这里的期望数值并加逃逸场景测试。**
- 跨 crate / LTO 安全性靠 `tests/run-make/`（可多 crate、真链接、真运行）。
- 纪律：新测试必须先验证「有区分度」（该失败的先失败）。

## 延伸阅读

- rustc-dev-guide「Compiletest」：`src/doc/rustc-dev-guide/src/tests/compiletest.md`
- 「Test directives」：`src/doc/rustc-dev-guide/src/tests/directives.md`
- 现有测试：`tests/codegen-llvm/virtual-function-elimination.rs`、`...-32bit.rs`、`tests/ui/codegen/virtual-function-elimination.rs`

下一篇：[`10-debugging-and-tools.md`](10-debugging-and-tools.md) —— 出问题时怎么打日志、看 MIR、导 LLVM IR、定位 ICE。
