//! These helpers provide a way to easily emulate a vectorized SIMD op on two SIMD vectors,
//! except using scalar ops that iterate through each lane, one at a time, so as to remove
//! the vagaries of compilation.
//!
//! Do note, however, that when testing that vectorized operations #[should_panic], these
//! "scalarized SIMD ops" will trigger scalar code paths that may also normally panic.

pub fn apply_unary_lanewise<T1: Copy, T2: Copy, V1: AsRef<[T1]>, V2: AsMut<[T2]> + Default>(
    x: V1,
    f: impl Fn(T1) -> T2,
) -> V2 {
    let mut y = V2::default();
    assert_eq!(x.as_ref().len(), y.as_mut().len());
    for (x, y) in x.as_ref().iter().zip(y.as_mut().iter_mut()) {
        *y = f(*x);
    }
    y
}

pub fn apply_binary_lanewise<T: Copy, V: AsRef<[T]> + AsMut<[T]> + Default>(
    a: V,
    b: V,
    f: impl Fn(T, T) -> T,
) -> V {
    let mut out = V::default();
    let out_slice = out.as_mut();
    let a_slice = a.as_ref();
    let b_slice = b.as_ref();
    for (o, (a, b)) in out_slice.iter_mut().zip(a_slice.iter().zip(b_slice.iter())) {
        *o = f(*a, *b);
    }
    out
}

pub fn apply_binary_scalar_rhs_lanewise<T: Copy, V: AsRef<[T]> + AsMut<[T]> + Default>(
    a: V,
    b: T,
    f: impl Fn(T, T) -> T,
) -> V {
    let mut out = V::default();
    let out_slice = out.as_mut();
    let a_slice = a.as_ref();
    for (o, a) in out_slice.iter_mut().zip(a_slice.iter()) {
        *o = f(*a, b);
    }
    out
}

pub fn apply_binary_scalar_lhs_lanewise<T: Copy, V: AsRef<[T]> + AsMut<[T]> + Default>(
    a: T,
    b: V,
    f: impl Fn(T, T) -> T,
) -> V {
    let mut out = V::default();
    let out_slice = out.as_mut();
    let b_slice = b.as_ref();
    for (o, b) in out_slice.iter_mut().zip(b_slice.iter()) {
        *o = f(a, *b);
    }
    out
}
