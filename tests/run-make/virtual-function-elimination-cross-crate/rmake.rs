// Regression guard for the `-Zvirtual-function-elimination` (VFE) miscompilation
// risk tracked by rust-lang/rust#68262 and documented under "Limitations" in
// `src/doc/unstable-book/src/compiler-flags/virtual-function-elimination.md`.
//
// `foo.rs` defines a *private* trait `Foo` whose `dyn Foo` vtable nevertheless
// escapes to downstream crates through a public constructor (`make_foo`) and an
// `#[inline]` caller (`f`). `main.rs` performs the virtual call from a separate
// crate. Everything is linked with fat LTO so LLVM's whole-program
// devirtualization / VFE passes actually run.
//
// If VFE narrows `Foo`'s `!vcall_visibility` too aggressively (treating the
// private trait as unreachable outside its crate) and LLVM removes `Foo::foo`,
// this program crashes or prints the wrong value instead of `42`. Asserting the
// runtime result keeps that miscompilation from silently returning.
//
// See `rustc-onboarding-68262/roadmap/05-detailed-tasks.md` task T2.1.

//@ ignore-cross-compile
// Reason: the compiled binary is executed.
//@ only-x86_64
// Reason: VFE metadata (typeid vtable offsets) is validated on x86_64.
//@ ignore-backends: gcc
// Reason: the GCC backend does not implement `llvm.type.checked.load`.

use run_make_support::{run, rustc};

fn main() {
    // Build the upstream rlib with VFE + fat LTO so its `dyn Foo` vtable is
    // emitted with `!type` / `!vcall_visibility` metadata.
    rustc()
        .input("foo.rs")
        .crate_type("rlib")
        .crate_name("foo")
        .edition("2021")
        .arg("-Zvirtual-function-elimination")
        .lto("fat")
        .opt_level("3")
        .symbol_mangling_version("v0")
        .run();

    // Link the downstream binary with fat LTO so LLVM runs WPD/VFE over the whole
    // program, including the upstream rlib's vtable.
    rustc()
        .input("main.rs")
        .extern_("foo", "libfoo.rlib")
        .edition("2021")
        .arg("-Zvirtual-function-elimination")
        .lto("fat")
        .opt_level("3")
        .symbol_mangling_version("v0")
        .run();

    run("main").assert_stdout_equals("42");
}
