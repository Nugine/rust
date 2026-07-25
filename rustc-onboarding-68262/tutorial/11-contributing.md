# 11 · 贡献流程

目标：把「改动怎么变成合入 rust-lang/rust 的 PR」这条路走通——PR 规范、review、`r+`、bors 合并队列、CI、以及针对本任务（不稳定编译器特性）的特殊注意点。

> 说明：本手册所在仓库是 `Nugine/rust`（一个 fork）。真正要贡献给上游时，流程面向 `rust-lang/rust`。下面讲的是上游流程，本地开发同样适用。

## 11.1 分支与提交

- 从最新的 `master` 开一个特性分支开发。
- 提交信息写清楚「做了什么、为什么」。一个 PR 可以有多个 commit，review 完可能被要求 squash。
- **每个 commit 尽量能独立编译通过**（方便二分定位）。本任务天然适合拆成：先加测试 → 再改可见性逻辑 → 再放开更多场景。

## 11.2 提 PR 前的本地自检

对应教程 04/09 的清单，最少：

```bash
./x.py fmt                # 格式化
./x.py check              # 整体编过
./x.py test tidy          # 规范/许可证头/文档检查（CI 必跑）
./x.py test tests/codegen-llvm/virtual-function-elimination.rs
./x.py test tests/ui/codegen/virtual-function-elimination.rs
```

- **tidy** 会检查行尾、许可证头、特性门、以及一些文档一致性。本地先过，避免 CI 反复红。
- 如果你改了 `-Z` 开关的行为或加了新诊断，往往要 `--bless` 更新 ui 快照，并把更新后的 `.stderr` 一并提交。

## 11.3 review 与 `r+`（bors）

rust-lang/rust 用一个叫 **bors**（这个仓库里也有 `rust-bors.toml`）的合并机器人：

1. 你开 PR 后，triagebot 会按改动路径**自动指派 reviewer**（`triagebot.toml` 配置了 codegen 相关的 owner）。
2. reviewer 评论、你迭代。
3. reviewer 满意后回复 `@bors r+`（或 `r=someone`），PR 进入**合并队列**。
4. bors **在合并前对每个 PR 单独跑全量 CI**，全绿才真正合并到 `master`。这保证 master 永远是绿的。
5. 大改动可能先 `@bors try` 触发一次试运行 + perf 基准（本任务若影响 codegen 性能，可能被要求跑 perf）。

对新人的实用建议：
- 主动在 PR 描述里写清**动机、方案、风险、测试覆盖了什么**——本任务尤其要说明「为什么这样改是保守/安全的」。
- 涉及 miscompile 风险的改动，reviewer 会非常关注**正确性论证**，把你的推理写进 PR 和测试注释里。

## 11.4 本任务的特殊流程注意点

#68262 涉及**不稳定编译器特性**（`-Zvirtual-function-elimination`）和**潜在 miscompile**，有几条额外规矩：

1. **不稳定开关不需要 RFC/FCP 就能改行为**，因为它本就没有稳定性承诺。但**语义变化要更新文档**：`src/doc/unstable-book/src/compiler-flags/virtual-function-elimination.md`。该文档目前记录了「可能过度删除函数」的已知限制——你的保守化修复应当同步更新这里。
2. **正确性优先于优化收益**：把某些场景从 `TranslationUnit(2)` 降级到 `LinkageUnit(1)` 或 `Public(0)`，即使损失优化，也会被接受——只要论证充分。
3. **可能需要 T-compiler / codegen 团队的意见**：涉及可见性语义、跨 crate、LTO 边界这类设计问题，值得在 issue #68262 或 Zulip 的 `t-compiler` / `wg-llvm` 频道先讨论方案，再动大手术。
4. **性能回归敏感**：如果改动让更多 vtable 落到保守可见性，可能影响 codegen 产物大小/性能；反之放开更多场景可能有正确性风险。两个方向都可能触发 perf run。

## 11.5 和上游同步

- 上游演进快，长期分支要定期 rebase 到最新 `master`（本手册里所有行号都会随之漂移——用符号名 grep 复位）。
- rustc-dev-guide 本身也在这个仓库（`src/doc/rustc-dev-guide/`），是你随时可查的权威。

## 11.6 从「读完教程」到「提第一个 PR」的最短路径

把整套教程落到行动上，推荐的第一个 PR **不是**去实现完整 WPD，而是**低风险、高价值**的一步（详见 roadmap/04 阶段一、二）：

1. 按 roadmap/05「阶段一」用 10.7 的六个 case 建立现状对照表；
2. 挑一个**已知有 miscompile 风险**的场景（如私有 trait 经 public wrapper + inline 逃逸）；
3. 写一个 run-make 测试**暴露**这个风险（先让它以 miscompile/失败的方式复现，或至少锁定当前危险数值）；
4. 如果确实能触发，把对应场景的 `vcall_visibility` 保守降级；
5. 更新 codegen 测试期望值 + unstable-book 文档；
6. 提 PR，在描述里讲清「这是修正一个潜在 miscompile，代价是少量优化机会」。

这样的第一个 PR 目标清晰、争议小、易于 review，是新人切入 #68262 的理想入口。

## 11.7 本篇小结

- 流程：特性分支 → 本地 fmt/check/tidy/相关测试 → 开 PR → reviewer → `@bors r+` → 合并队列全量 CI → 合并。
- 本任务特殊点：改不稳定开关行为要更新 unstable-book；正确性优先于优化；设计问题先讨论；对 perf 敏感。
- 第一个 PR 建议做「保守化 + 回归测试」，别一上来啃完整 WPD。

## 延伸阅读

- `CONTRIBUTING.md`（仓库根）
- rustc-dev-guide「Contributing / Getting started」：`src/doc/rustc-dev-guide/src/getting-started.md`
- rustc-dev-guide「About bors」等 CI 章节
- 在线：<https://rustc-dev-guide.rust-lang.org/getting-started.html>

—— 教程部分到此结束。接下来请转到 [`../roadmap/00-index.md`](../roadmap/00-index.md)，把这些知识用到 #68262 的具体实施上。
