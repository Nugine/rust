// The upstream crate. `Foo` is a *private* trait, but a `dyn Foo` trait object
// escapes across the crate boundary:
//
//   * `make_foo` (public) constructs and returns a `Box<dyn Foo>` (wrapped in a
//     public newtype), so the vtable value leaks to downstream crates, and
//   * `f` is `#[inline]`, so its body -- including the virtual call `a.0.foo()`
//     -- is codegen'd *inside* downstream crates.
//
// This is the scenario documented as a known miscompilation risk in
// `src/doc/unstable-book/src/compiler-flags/virtual-function-elimination.md`:
// because `Foo` is private, `-Zvirtual-function-elimination` currently marks its
// vtable with a narrow `!vcall_visibility`, even though `Foo::foo` can be called
// from a foreign crate through the inlined `f`. If VFE deletes `Foo::foo` (or WPD
// devirtualizes unsoundly), the call below jumps to a removed function.

trait Foo {
    fn foo(&self) -> u32 {
        42
    }
}

impl Foo for usize {}

pub struct FooBox(Box<dyn Foo>);

pub fn make_foo() -> FooBox {
    FooBox(Box::new(0usize))
}

#[inline]
pub fn f(a: FooBox) -> u32 {
    a.0.foo()
}
