// The downstream crate. It cannot name the private `Foo` trait, but it can still
// trigger a virtual call on it by going through the public `make_foo` + inlined
// `f` from the upstream crate. If `Foo::foo` was removed too eagerly, this call
// misbehaves (crash or wrong result) instead of returning `42`.

fn main() {
    let result = foo::f(foo::make_foo());
    // If `Foo::foo` was eliminated/devirtualized unsoundly across the crate
    // boundary, this is where it shows up as a crash or a wrong value.
    assert_eq!(result, 42);
    print!("{result}");
}
