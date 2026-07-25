# 08 · codegen 与 LLVM 交互（改动落点篇）

目标：讲清楚 rustc 后端的两层结构（后端无关的 `rustc_codegen_ssa` vs LLVM 专属的 `rustc_codegen_llvm`）、intrinsic 是怎么发出的、怎么给 LLVM global 挂 metadata。读完你应该能对着 roadmap/03 的代码地图，准确说出「每一处改动在哪一层、为什么在那一层」。

## 8.1 两层结构：ssa 骨架 + llvm 后端

rustc 支持多个 codegen 后端（LLVM / GCC / Cranelift），为了复用，把 codegen 拆成两层：

- **`rustc_codegen_ssa`（后端无关骨架）**：遍历 MIR，把每个操作翻译成对一组**抽象 trait** 的调用。它不知道 LLVM 的存在，只知道「有个 builder，能 `load`、能 `call`、能 `type_checked_load`」。
- **`rustc_codegen_llvm`（LLVM 后端）**：实现那些抽象 trait，真正调用 LLVM C++ API（经 `rustc_llvm` FFI）产出 LLVM IR。

这些抽象 trait 定义在 `rustc_codegen_ssa/src/traits/`，关键几个：

| trait | 含义 | 本任务相关方法 |
| --- | --- | --- |
| `BuilderMethods` | 在函数体里发指令的 builder | `load`、`call`、**`type_checked_load`** |
| `CodegenMethods` / `MiscCodegenMethods` | 模块级 codegen 能力 | **`apply_vcall_visibility_metadata`** |
| `ConstCodegenMethods` | 生成常量 | `static_addr_of`（生成 vtable global） |

> 记这条规则：**「后端无关的决策」放 ssa，「LLVM 特有的发射」放 llvm。** 本任务里，「要不要走 checked load」「该挂什么可见性」这类**语义决策**倾向放 ssa（或 `rustc_middle` 的 query），而「怎么发 `!type` 元数据 / `llvm.type.checked.load`」这类**LLVM 具体动作**放 llvm。这直接影响你把新逻辑写在哪。

## 8.2 vtable 的生成：从 ssa 到 llvm

回顾教程 07 的 `get_vtable`（`rustc_codegen_ssa/src/meth.rs:103`），它是**后端无关**的：

```
get_vtable (ssa)
  ├─ tcx.vtable_allocation((ty, trait_ref))     ← query，拿 vtable 常量数据
  ├─ cx.static_addr_of(...)                      ← 生成 global（llvm 实现）
  ├─ cx.apply_vcall_visibility_metadata(...)     ← 挂元数据（llvm 实现）
  └─ cx.create_vtable_debuginfo(...)             ← debuginfo（llvm 实现）
```

`get_vtable` 本身在 ssa 层，但它调用的 `apply_vcall_visibility_metadata` 是通过 trait 分派到 **llvm 层的具体实现**（`rustc_codegen_llvm/src/debuginfo/metadata.rs:1691`）。这就是「骨架在 ssa、发射在 llvm」的典型样子。

## 8.3 给 LLVM global 挂 metadata 的机制

LLVM 里，一个全局对象（如 vtable）可以携带**具名元数据节点**。rustc 在 `apply_vcall_visibility_metadata`（已核对源码）里这样挂：

```rust
// !type：offset=0 + typeid（把这个 vtable 归类到某个 type identifier）
let type_ = [llvm::LLVMValueAsMetadata(cx.const_usize(0)), typeid];
cx.global_add_metadata_node(vtable, llvm::MD_type, &type_);

// !vcall_visibility：0/1/2（Public/LinkageUnit/TranslationUnit）
let vcall_visibility = [llvm::LLVMValueAsMetadata(cx.const_u64(vcall_visibility as u64))];
cx.global_set_metadata_node(vtable, llvm::MD_vcall_visibility, &vcall_visibility);
```

要点：

- **`MD_type` / `MD_vcall_visibility`** 是 LLVM 预定义的 metadata kind id（在 `rustc_codegen_llvm` 里有对应常量）。
- `!type` 的第一个元素是 offset（这里 0），第二个是 typeid（一段字节，来自 mangling）。offset=0 对应 Itanium ABI 里 vtable 的 "address point" 建模（PR #96285 的提交说明有解释）。
- `global_add_metadata_node`（**add**，可挂多个）vs `global_set_metadata_node`（**set**，覆盖式）：一个 vtable 可以有多条 `!type`（比如同时属于多个 typeid），但 `!vcall_visibility` 是唯一的。

> 对本任务：**你几乎不需要改「怎么挂」，而要改「挂什么值」**——即那个 `vcall_visibility` 变量怎么算出来。这让改动更集中、更安全。

## 8.4 调用点：`type_checked_load` intrinsic

虚调用点通过 `load_vtable`（ssa，教程 07）决定走普通 load 还是 checked load。走 checked load 时，落到 llvm 后端的 `type_checked_load`（`rustc_codegen_llvm/src/intrinsic.rs:1048`，已核对）：

```rust
fn type_checked_load(&mut self, llvtable, vtable_byte_offset: u64, typeid: &[u8]) -> Value {
    let typeid = self.get_metadata_value(self.create_metadata(typeid));
    let vtable_byte_offset = self.const_i32(vtable_byte_offset as i32);
    let ret = self.call_intrinsic("llvm.type.checked.load", &[], &[llvtable, vtable_byte_offset, typeid]);
    self.extract_value(ret, 0)  // 只取返回结构体的第 0 个：函数指针
}
```

- `llvm.type.checked.load(vtable, offset, typeid)` 返回 `{ptr, i1}`：函数指针 + 一个「类型检查是否通过」的布尔。rustc 只取第 0 个（函数指针）。
- **它的意义**：普通 `load` 对 LLVM 的去虚化是「不透明」的；`llvm.type.checked.load` 显式告诉 LLVM「这个 load 是从某 typeid 的 vtable 的某 offset 取函数」，LLVM 才能把调用点和候选 vtable 关联，进而在 `WholeProgramDevirt` 里做静态化。
- **GCC 后端**：`rustc_codegen_gcc` 的对应实现是占位（不支持该 intrinsic）。**本任务要限定 LLVM 后端**，否则在 GCC 后端会静默走错路径。

## 8.5 typeid 从哪来：符号 mangling

`!type` 和 `type_checked_load` 用的 typeid，来自 `rustc_symbol_mangling`：

- 入口 `typeid_for_trait_ref(tcx, trait_ref)`（`rustc_symbol_mangling/src/lib.rs:145`，已核对），内部调 `v0::mangle_typeid_for_trait_ref`。
- 它把 existential trait ref 编码成一个稳定字符串（v0 mangling），作为 LLVM type identifier。**同一个 trait ref → 同一个 typeid**，这样「一个调用点」和「所有该 trait 的 vtable」才能在 LLVM 里对上号。
- 这解释了为什么现有 codegen 测试要加 `-Csymbol-mangling-version=v0`：typeid 依赖 v0 mangling。

## 8.6 模块级开关：`"Virtual Function Elim"` flag

`rustc_codegen_llvm/src/context.rs:471`（已核对）在开了 `-Zvirtual-function-elimination` 时给 LLVM module 设一个 flag：

```rust
llvm::add_module_flag_u32(llmod, ModuleFlagMergeBehavior::Error, "Virtual Function Elim", 1);
```

- 这是 LLVM 的 `GlobalDCE`/VFE 的总开关。
- `MergeBehavior::Error` 意味着：如果 LTO 把多个 module 合并、而它们对这个 flag 取值不一致，链接会**报错而不是静默错误优化**——一种安全阀。

## 8.7 命令行开关与校验

- **定义**：`-Zvirtual-function-elimination` 在 `rustc_session/src/options.rs:2961`（`virtual_function_elimination: bool`，`[TRACKED]` 表示它影响增量编译缓存 key）。
- **校验**：`rustc_session/src/session.rs:1559` 附近，若开了该 flag 但不是 fat LTO，会报错。这保证了「元数据生成、checked load、可见性判断」这三处对 `Lto::Fat` 的假设一致。

## 8.8 把「改动落点」汇成一张层次图

```
rustc_session         定义 -Zvirtual-function-elimination + 校验需 fat LTO
      │
rustc_middle          Ty / TyKind::Dynamic / vtable_entries / visibility (query)
      │               ←（本任务可能在这里新增「vtable 可达性」query）
rustc_symbol_mangling typeid_for_trait_ref（v0）
      │
rustc_codegen_ssa     get_vtable / load_vtable（后端无关决策）
      │               ←（本任务的「该挂什么可见性」决策可上提到这里或 middle）
rustc_codegen_llvm    apply_vcall_visibility_metadata / type_checked_load / module flag
                      ←（本任务「挂什么值」的直接改动点）
```

**一句话记住**：**LLVM 侧的「怎么发」已经完备；本任务是让 rustc 侧「发什么可见性值」变得保守而正确**，落点集中在 `apply_vcall_visibility_metadata` 的可见性计算，以及可能新增的 `rustc_middle` query。

## 8.9 本篇小结

- codegen 两层：`rustc_codegen_ssa`（后端无关骨架，走 trait）+ `rustc_codegen_llvm`（LLVM 发射）。
- 挂 metadata：`global_add_metadata_node(MD_type)` + `global_set_metadata_node(MD_vcall_visibility)`；你改的是「值」不是「机制」。
- `llvm.type.checked.load` 让虚调用点对 LLVM 去虚化透明；GCC 后端不支持，需限定 LLVM。
- typeid 来自 v0 mangling（`typeid_for_trait_ref`），故测试需 `-Csymbol-mangling-version=v0`。
- 开关在 `rustc_session`，`[TRACKED]` 且要求 fat LTO。

## 延伸阅读

- rustc-dev-guide「Code generation / Backend」：`src/doc/rustc-dev-guide/src/backend/codegen.md`
- LLVM Type Metadata：<https://llvm.org/docs/TypeMetadata.html>
- LLVM `llvm.type.checked.load` intrinsic 文档（LLVM LangRef）
- 源码：`compiler/rustc_codegen_ssa/src/meth.rs`、`compiler/rustc_codegen_llvm/src/debuginfo/metadata.rs`、`.../intrinsic.rs`、`.../context.rs`

下一篇：[`09-testing.md`](09-testing.md) —— 本任务大量工作是写测试，这一篇讲 compiletest、codegen(FileCheck)、ui、run-make 怎么用。
