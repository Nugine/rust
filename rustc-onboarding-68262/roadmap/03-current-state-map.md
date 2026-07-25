# 03 · 现状代码地图

目标：给出**已逐条核对**的「文件 + 符号 + 行号 + 职责」清单，作为你动手时的导航。行号以本仓库当前状态为准（rustc 1.99.0）；上游会漂移，**用符号名 grep 复位**。

> 核对方式：本篇每个坐标都用 `grep`/`view` 在本仓库当前源码上确认过（见本 PR 提交历史中的探查）。

## 3.1 一图总览：改动落在哪一层

```
rustc_session      ── 开关定义 + fat LTO 校验
rustc_middle       ── Ty/TyKind::Dynamic、vtable_entries、visibility (query)   ← 可能新增可达性 query
rustc_symbol_mangling ── typeid_for_trait_ref (v0)
rustc_codegen_ssa  ── get_vtable / load_vtable（后端无关决策）
rustc_codegen_llvm ── apply_vcall_visibility_metadata / type_checked_load / module flag  ← 直接改动点
```

## 3.2 精确坐标清单（已核对）

### 开关与校验（rustc_session）

| 文件:行 | 符号 | 职责 |
| --- | --- | --- |
| `compiler/rustc_session/src/options.rs:2961` | `virtual_function_elimination: bool` | 定义 `-Zvirtual-function-elimination`，标记 `[TRACKED]`（影响增量缓存 key） |
| `compiler/rustc_session/src/session.rs:1557-1562` | fat LTO 校验 | 若开了该 flag 但 `sess.lto() != Lto::Fat`，`emit_err(UnstableVirtualFunctionElimination)` |

### vtable 创建与虚调用（rustc_codegen_ssa）

| 文件:行 | 符号 | 职责 |
| --- | --- | --- |
| `compiler/rustc_codegen_ssa/src/meth.rs:103` | `get_vtable(cx, ty, trait_ref)` | 创建/缓存 vtable global；调 `tcx.vtable_allocation`、`static_addr_of`、`apply_vcall_visibility_metadata`、`create_vtable_debuginfo` |
| `compiler/rustc_codegen_ssa/src/meth.rs:126` | `load_vtable(...)` | 从 vtable 取 slot；**若 VFE + `Lto::Fat`** 且有 principal trait，走 `type_checked_load`（typeid 来自 `typeid_for_trait_ref`）；否则普通 `load` |
| `compiler/rustc_codegen_ssa/src/meth.rs:139` | `dyn_trait_in_self(tcx, ty)` | 从类型里取出 principal `ExistentialTraitRef`（决定 typeid） |

### 元数据发射（rustc_codegen_llvm）

| 文件:行 | 符号 | 职责 |
| --- | --- | --- |
| `compiler/rustc_codegen_llvm/src/debuginfo/metadata.rs:1691` | `apply_vcall_visibility_metadata(cx, ty, trait_ref, vtable)` | **本任务核心**。未开 VFE 或非 fat LTO 直接返回；否则由 `(lto, trait_vis, single_cgu)` 算出 `VCallVisibility`，挂 `!type`(offset 0 + typeid) 和 `!vcall_visibility` |
| `.../metadata.rs:1703-1707` | `enum VCallVisibility { Public=0, LinkageUnit=1, TranslationUnit=2 }` | 三档可见性 |
| `.../metadata.rs:1715` | `let trait_vis = cx.tcx.visibility(trait_def_id)` | **当前的近似来源**（本任务要替换/收紧的点） |
| `.../metadata.rs:1717-1718` | `single_cgu = codegen_units == 1` | 参与判定 |
| `.../metadata.rs:1724-1740` | `match (lto, trait_vis, single_cgu)` | 具体判定表（见 3.3） |
| `.../metadata.rs:1745-1749` | `global_add_metadata_node(MD_type)` / `global_set_metadata_node(MD_vcall_visibility)` | 真正挂元数据 |
| `compiler/rustc_codegen_llvm/src/intrinsic.rs:1048` | `type_checked_load(llvtable, offset, typeid)` | 发出 `llvm.type.checked.load`，取返回结构第 0 项（函数指针） |
| `compiler/rustc_codegen_llvm/src/context.rs:471-478` | module flag | 开 VFE 时设 `"Virtual Function Elim" = 1`（`MergeBehavior::Error`） |

### typeid 生成（rustc_symbol_mangling）

| 文件:行 | 符号 | 职责 |
| --- | --- | --- |
| `compiler/rustc_symbol_mangling/src/lib.rs:145` | `typeid_for_trait_ref(tcx, trait_ref)` | 入口，转调 v0 |
| `compiler/rustc_symbol_mangling/src/v0.rs`（`mangle_typeid_for_trait_ref`） | v0 mangling | 把 existential trait ref 编码成稳定 typeid 字符串 |

### 相关 query / 类型（rustc_middle）

| 文件:行 | 符号 | 职责 |
| --- | --- | --- |
| `compiler/rustc_middle/src/queries.rs:2177` | `query visibility(def_id) -> Visibility<ModId>` | 名字可见性（当前近似所用） |
| `compiler/rustc_middle/src/queries.rs:1569` | `query vtable_entries(trait_ref) -> &[VtblEntry]` | vtable 布局 |
| `compiler/rustc_middle/src/queries.rs:1563` | `query own_existential_vtable_entries(...)` | vtable 自有项 |
| `compiler/rustc_middle/src/ty/vtable.rs` | `enum VtblEntry` / `COMMON_VTABLE_ENTRIES` | vtable 项类型（Method/TraitVPtr/Metadata…） |
| `compiler/rustc_middle/src/ty/mod.rs:386` | `enum Visibility<Id>` | `Public` / `Restricted(mod)` |

### 后端差异（rustc_codegen_gcc）

| 位置 | 现状 |
| --- | --- |
| `compiler/rustc_codegen_gcc/src/intrinsic/mod.rs`（`type_checked_load`） | **占位实现**，不支持 `llvm.type.checked.load`。**本任务须限定 LLVM 后端**，避免 GCC 路径误用 |

### 文档与测试

| 位置 | 内容 |
| --- | --- |
| `src/doc/unstable-book/src/compiler-flags/virtual-function-elimination.md` | 用户文档 + **已承认的 miscompile 限制**（私有 trait + public wrapper + `#[inline]`，见 01/07） |
| `tests/codegen-llvm/virtual-function-elimination.rs` | 64 位 codegen 金标准（元数据 + checked load + vcall_visibility 数值 + 未用函数删除） |
| `tests/codegen-llvm/virtual-function-elimination-32bit.rs` | 32 位 offset 变体 |
| `tests/ui/codegen/virtual-function-elimination.rs` | build-pass 回归 |

## 3.3 当前 `!vcall_visibility` 判定表（逐字核对源码）

`apply_vcall_visibility_metadata` 里 `match (lto, trait_vis, single_cgu)`（`metadata.rs:1724-1740`）：

| lto | trait_vis | single_cgu | 结果 |
| --- | --- | --- | --- |
| `No` / `ThinLocal` | `Public` | 任意 | `Public (0)` |
| `No` | `Restricted` | false（多 CGU） | `Public (0)` |
| `Fat` / `Thin` | `Public` | 任意 | `LinkageUnit (1)` |
| `ThinLocal` / `Thin` / `Fat` | `Restricted` | false（多 CGU） | `LinkageUnit (1)` |
| 任意 | `Restricted` | true（单 CGU） | `TranslationUnit (2)` |

> 但别忘了函数**开头的早返回**：只有 `virtual_function_elimination && lto == Fat` 才会走到这张表（`metadata.rs:1699`）。所以**实际生效的只有 `lto == Fat` 那几行**——即 public→`LinkageUnit`，私有多 CGU→`LinkageUnit`，私有单 CGU→`TranslationUnit`。这与现有测试断言（私有 `T`→2、公开 `V`→1）一致。

**本任务的靶心就在这张表**：私有 trait 被判 `TranslationUnit(2)` / `LinkageUnit(1)` 的那两行，正是 01/07 的 miscompile 场景来源。因为判据只看 `trait_vis`（名字可见性）+ CGU 数，**完全没考虑 vtable 是否经 wrapper/inline/泛型逃逸**。

## 3.4 数据流：一次虚调用从类型到元数据

```
&dyn Animal (TyKind::Dynamic, principal = Animal)
   │ get_vtable (meth.rs:103)
   │   tcx.vtable_allocation((ty, trait_ref)) → vtable 常量
   │   static_addr_of → LLVM global
   ▼
apply_vcall_visibility_metadata (metadata.rs:1691)
   │   trait_def_id = trait_ref.with_self_ty(ty).def_id
   │   trait_vis = tcx.visibility(trait_def_id)      ← ★ 唯一的可见性输入
   │   match (lto, trait_vis, single_cgu) → 0/1/2
   │   挂 !type(0,typeid) + !vcall_visibility(n)
   ▼
调用点 load_vtable (meth.rs:126)
   │   若 VFE+Fat: type_checked_load → intrinsic.rs:1048 → llvm.type.checked.load
   ▼
LLVM LTO: WholeProgramDevirt / GlobalDCE 消费上述元数据
```

★ 标记处就是本任务要动的语义输入。

## 3.5 本篇小结

- rustc 侧机制分布在 5 个 crate，坐标已核对（见 3.2）。
- 唯一决定 `!vcall_visibility` 的语义输入是 `tcx.visibility(trait_def_id)` + CGU 数（`metadata.rs:1715/1724`），**没有逃逸/可达性判断** → 这是缺口所在。
- 实际生效判定（fat LTO）：public→LinkageUnit、私有多CGU→LinkageUnit、私有单CGU→TranslationUnit。
- GCC 后端不支持 checked load；须限定 LLVM。

下一篇：[`04-phased-plan.md`](04-phased-plan.md)
