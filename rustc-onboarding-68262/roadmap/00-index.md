# #68262 实施路线 · 总览（00 · Index）

本目录把 rust-lang/rust#68262（whole program devirtualization，整程序去虚化）从「一份调查结论」推进为「可执行的实施计划」。它建立在两份前置材料之上：

- 上层调查报告：[`../../issue-68262-investigation.md`](../../issue-68262-investigation.md)（结论、代码地图、风险）；
- rustc 上手教程：[`../tutorial/`](../tutorial/00-index.md)（若你不熟 rustc，先读它，尤其 07/08/09）。

> 术语：**WPD** = 把「实际会被调用的虚调用」静态化为直接调用（issue 的真正目标）；**VFE** = 删除「从不被动态调用的 vtable 函数项」（PR #96285 已实现的基础设施）。两者共享 LLVM 元数据但目标不同，详见 02 与 08。

## 一句话结论（先给你锚点）

**#68262 的难点不在「怎么发 LLVM 元数据」（已完备），而在「rustc 如何保守、正确地判断某个 vtable 的使用边界（`!vcall_visibility`）」。当前实现用 `tcx.visibility(trait_def_id)` 近似，这在私有 trait 经 public wrapper / `#[inline]` / 泛型逃逸时会导致 miscompile。第一优先级是把这些危险场景固化成测试并保守化，而不是急着开启更多优化。**

## 各文档导读

| 篇号 | 标题 | 内容 |
| --- | --- | --- |
| 01 | 问题陈述 | issue 到底要什么、VFE vs WPD、验收标准 |
| 02 | LLVM 背景 | `!type` / `!vcall_visibility` / `type.checked.load` / WPD pass 怎么协作 |
| 03 | 现状代码地图 | 已核对的文件+符号清单，每处做什么、为什么在那 |
| 04 | 分阶段计划 | 五个阶段：复现 → 保守化回归 → 可达性分析 → 可观测去虚化 → ThinLTO |
| 05 | 详细任务分解 | 每个阶段拆成带验收标准的具体任务，可直接排期 |
| 06 | 测试策略 | codegen / ui / run-make 各测什么、怎么写、怎么保证有区分度 |
| 07 | 风险与未决问题 | miscompile 边界清单、需要团队决策的开放问题 |
| 08 | 术语表 | VFE/WPD/vcall_visibility/typeid/CGU/LTO 等一站式解释 |

## 建议执行顺序

1. 读 01 → 02 → 03，建立「要什么 + LLVM 怎么用 + rustc 现在怎么做」的完整图景。
2. 读 04 → 05，明确阶段与任务。
3. 动手时以 05 的「阶段一」为起点，配合 06 的测试策略。
4. 任何涉及「能不能缩窄可见性」的判断，回查 07 的风险清单。

## 核心原则（贯穿所有阶段）

1. **正确性 > 优化收益**：拿不准就回退到更保守的 `!vcall_visibility`（`Public`）。少优化不是 bug，误删函数是 miscompile。
2. **先测试后实现**：本任务的第一份价值是「把已知 miscompile 风险变成可复现的测试」。
3. **区分 VFE 与 WPD**：不要因为 VFE 基础设施已存在就认为 issue 已解决；要有测试证明**真正的去虚化**在安全条件下发生。
4. **限定 fat LTO + LLVM 后端**：现状假设如此；ThinLTO、GCC 后端都放到最后或排除。
5. **可见性 ≠ 可达性**：`tcx.visibility` 说的是「名字能否被命名」，本任务要的是「vtable 会否被外部调用」。

下一篇：[`01-problem-statement.md`](01-problem-statement.md)
