use expect_test::expect;

use super::{check_infer, check_infer_with_mismatches, check_no_mismatches, check_types};

#[test]
fn infer_block_expr_type_mismatch() {
    check_infer(
        r"
        fn test() {
            let a: i32 = { 1i64 };
        }
        ",
        expect![[r"
            10..40 '{     ...4 }; }': ()
            20..21 'a': i32
            29..37 '{ 1i64 }': i64
            31..35 '1i64': i64
        "]],
    );
}

#[test]
fn coerce_places() {
    check_infer(
        r#"
//- minicore: coerce_unsized
struct S<T> { a: T }

fn f<T>(_: &[T]) -> T { loop {} }
fn g<T>(_: S<&[T]>) -> T { loop {} }

fn gen<T>() -> *mut [T; 2] { loop {} }
fn test1<U>() -> *mut [U] {
    gen()
}

fn test2() {
    let arr: &[u8; 1] = &[1];

    let a: &[_] = arr;
    let b = f(arr);
    let c: &[_] = { arr };
    let d = g(S { a: arr });
    let e: [&[_]; 1] = [arr];
    let f: [&[_]; 2] = [arr; 2];
    let g: (&[_], &[_]) = (arr, arr);
}
"#,
        expect![[r#"
            30..31 '_': &[T]
            44..55 '{ loop {} }': T
            46..53 'loop {}': !
            51..53 '{}': ()
            64..65 '_': S<&[T]>
            81..92 '{ loop {} }': T
            83..90 'loop {}': !
            88..90 '{}': ()
            121..132 '{ loop {} }': *mut [T; 2]
            123..130 'loop {}': !
            128..130 '{}': ()
            159..172 '{     gen() }': *mut [U]
            165..168 'gen': fn gen<U>() -> *mut [U; 2]
            165..170 'gen()': *mut [U; 2]
            185..419 '{     ...rr); }': ()
            195..198 'arr': &[u8; 1]
            211..215 '&[1]': &[u8; 1]
            212..215 '[1]': [u8; 1]
            213..214 '1': u8
            226..227 'a': &[u8]
            236..239 'arr': &[u8; 1]
            249..250 'b': u8
            253..254 'f': fn f<u8>(&[u8]) -> u8
            253..259 'f(arr)': u8
            255..258 'arr': &[u8; 1]
            269..270 'c': &[u8]
            279..286 '{ arr }': &[u8]
            281..284 'arr': &[u8; 1]
            296..297 'd': u8
            300..301 'g': fn g<u8>(S<&[u8]>) -> u8
            300..315 'g(S { a: arr })': u8
            302..314 'S { a: arr }': S<&[u8]>
            309..312 'arr': &[u8; 1]
            325..326 'e': [&[u8]; 1]
            340..345 '[arr]': [&[u8]; 1]
            341..344 'arr': &[u8; 1]
            355..356 'f': [&[u8]; 2]
            370..378 '[arr; 2]': [&[u8]; 2]
            371..374 'arr': &[u8; 1]
            376..377 '2': usize
            388..389 'g': (&[u8], &[u8])
            406..416 '(arr, arr)': (&[u8], &[u8])
            407..410 'arr': &[u8; 1]
            412..415 'arr': &[u8; 1]
        "#]],
    );
}

#[test]
fn infer_let_stmt_coerce() {
    check_infer(
        r"
        fn test() {
            let x: &[isize] = &[1];
            let x: *const [isize] = &[1];
        }
        ",
        expect![[r#"
            10..75 '{     ...[1]; }': ()
            20..21 'x': &[isize]
            34..38 '&[1]': &[isize; 1]
            35..38 '[1]': [isize; 1]
            36..37 '1': isize
            48..49 'x': *const [isize]
            68..72 '&[1]': &[isize; 1]
            69..72 '[1]': [isize; 1]
            70..71 '1': isize
        "#]],
    );
}

#[test]
fn infer_custom_coerce_unsized() {
    check_infer(
        r#"
//- minicore: coerce_unsized
use core::{marker::Unsize, ops::CoerceUnsized};

struct A<T: ?Sized>(*const T);
struct B<T: ?Sized>(*const T);
struct C<T: ?Sized> { inner: *const T }

impl<T: ?Sized + Unsize<U>, U: ?Sized> CoerceUnsized<B<U>> for B<T> {}
impl<T: ?Sized + Unsize<U>, U: ?Sized> CoerceUnsized<C<U>> for C<T> {}

fn foo1<T>(x: A<[T]>) -> A<[T]> { x }
fn foo2<T>(x: B<[T]>) -> B<[T]> { x }
fn foo3<T>(x: C<[T]>) -> C<[T]> { x }

fn test(a: A<[u8; 2]>, b: B<[u8; 2]>, c: C<[u8; 2]>) {
    let d = foo1(a);
    let e = foo2(b);
    let f = foo3(c);
}
"#,
        expect![[r#"
            306..307 'x': A<[T]>
            327..332 '{ x }': A<[T]>
            329..330 'x': A<[T]>
            344..345 'x': B<[T]>
            365..370 '{ x }': B<[T]>
            367..368 'x': B<[T]>
            382..383 'x': C<[T]>
            403..408 '{ x }': C<[T]>
            405..406 'x': C<[T]>
            418..419 'a': A<[u8; 2]>
            433..434 'b': B<[u8; 2]>
            448..449 'c': C<[u8; 2]>
            463..529 '{     ...(c); }': ()
            473..474 'd': A<[{unknown}]>
            477..481 'foo1': fn foo1<{unknown}>(A<[{unknown}]>) -> A<[{unknown}]>
            477..484 'foo1(a)': A<[{unknown}]>
            482..483 'a': A<[u8; 2]>
            494..495 'e': B<[u8]>
            498..502 'foo2': fn foo2<u8>(B<[u8]>) -> B<[u8]>
            498..505 'foo2(b)': B<[u8]>
            503..504 'b': B<[u8; 2]>
            515..516 'f': C<[u8]>
            519..523 'foo3': fn foo3<u8>(C<[u8]>) -> C<[u8]>
            519..526 'foo3(c)': C<[u8]>
            524..525 'c': C<[u8; 2]>
        "#]],
    );
}

#[test]
fn infer_if_coerce() {
    check_infer(
        r#"
//- minicore: unsize
fn foo<T>(x: &[T]) -> &[T] { loop {} }
fn test() {
    let x = if true {
        foo(&[1])
    } else {
        &[1]
    };
}
"#,
        expect![[r#"
            10..11 'x': &[T]
            27..38 '{ loop {} }': &[T]
            29..36 'loop {}': !
            34..36 '{}': ()
            49..125 '{     ...  }; }': ()
            59..60 'x': &[i32]
            63..122 'if tru...     }': &[i32]
            66..70 'true': bool
            71..96 '{     ...     }': &[i32]
            81..84 'foo': fn foo<i32>(&[i32]) -> &[i32]
            81..90 'foo(&[1])': &[i32]
            85..89 '&[1]': &[i32; 1]
            86..89 '[1]': [i32; 1]
            87..88 '1': i32
            102..122 '{     ...     }': &[i32; 1]
            112..116 '&[1]': &[i32; 1]
            113..116 '[1]': [i32; 1]
            114..115 '1': i32
        "#]],
    );
}

#[test]
fn infer_if_else_coerce() {
    check_infer(
        r#"
//- minicore: coerce_unsized
fn foo<T>(x: &[T]) -> &[T] { loop {} }
fn test() {
    let x = if true {
        &[1]
    } else {
        foo(&[1])
    };
}
"#,
        expect![[r#"
            10..11 'x': &[T]
            27..38 '{ loop {} }': &[T]
            29..36 'loop {}': !
            34..36 '{}': ()
            49..125 '{     ...  }; }': ()
            59..60 'x': &[i32]
            63..122 'if tru...     }': &[i32]
            66..70 'true': bool
            71..91 '{     ...     }': &[i32; 1]
            81..85 '&[1]': &[i32; 1]
            82..85 '[1]': [i32; 1]
            83..84 '1': i32
            97..122 '{     ...     }': &[i32]
            107..110 'foo': fn foo<i32>(&[i32]) -> &[i32]
            107..116 'foo(&[1])': &[i32]
            111..115 '&[1]': &[i32; 1]
            112..115 '[1]': [i32; 1]
            113..114 '1': i32
        "#]],
    )
}

#[test]
fn infer_match_first_coerce() {
    check_infer(
        r#"
//- minicore: unsize
fn foo<T>(x: &[T]) -> &[T] { loop {} }
fn test(i: i32) {
    let x = match i {
        2 => foo(&[2]),
        1 => &[1],
        _ => &[3],
    };
}
"#,
        expect![[r#"
            10..11 'x': &[T]
            27..38 '{ loop {} }': &[T]
            29..36 'loop {}': !
            34..36 '{}': ()
            47..48 'i': i32
            55..149 '{     ...  }; }': ()
            65..66 'x': &[i32]
            69..146 'match ...     }': &[i32]
            75..76 'i': i32
            87..88 '2': i32
            87..88 '2': i32
            92..95 'foo': fn foo<i32>(&[i32]) -> &[i32]
            92..101 'foo(&[2])': &[i32]
            96..100 '&[2]': &[i32; 1]
            97..100 '[2]': [i32; 1]
            98..99 '2': i32
            111..112 '1': i32
            111..112 '1': i32
            116..120 '&[1]': &[i32; 1]
            117..120 '[1]': [i32; 1]
            118..119 '1': i32
            130..131 '_': i32
            135..139 '&[3]': &[i32; 1]
            136..139 '[3]': [i32; 1]
            137..138 '3': i32
        "#]],
    );
}

#[test]
fn infer_match_second_coerce() {
    check_infer(
        r#"
//- minicore: coerce_unsized
fn foo<T>(x: &[T]) -> &[T] { loop {} }
fn test(i: i32) {
    let x = match i {
        1 => &[1],
        2 => foo(&[2]),
        _ => &[3],
    };
}
"#,
        expect![[r#"
            10..11 'x': &[T]
            27..38 '{ loop {} }': &[T]
            29..36 'loop {}': !
            34..36 '{}': ()
            47..48 'i': i32
            55..149 '{     ...  }; }': ()
            65..66 'x': &[i32]
            69..146 'match ...     }': &[i32]
            75..76 'i': i32
            87..88 '1': i32
            87..88 '1': i32
            92..96 '&[1]': &[i32; 1]
            93..96 '[1]': [i32; 1]
            94..95 '1': i32
            106..107 '2': i32
            106..107 '2': i32
            111..114 'foo': fn foo<i32>(&[i32]) -> &[i32]
            111..120 'foo(&[2])': &[i32]
            115..119 '&[2]': &[i32; 1]
            116..119 '[2]': [i32; 1]
            117..118 '2': i32
            130..131 '_': i32
            135..139 '&[3]': &[i32; 1]
            136..139 '[3]': [i32; 1]
            137..138 '3': i32
        "#]],
    );
}

#[test]
fn coerce_merge_one_by_one1() {
    cov_mark::check!(coerce_merge_fail_fallback);

    check_infer(
        r"
        fn test() {
            let t = &mut 1;
            let x = match 1 {
                1 => t as *mut i32,
                2 => t as &i32,
                _ => t as *const i32,
            };
        }
        ",
        expect![[r"
            10..144 '{     ...  }; }': ()
            20..21 't': &mut i32
            24..30 '&mut 1': &mut i32
            29..30 '1': i32
            40..41 'x': *const i32
            44..141 'match ...     }': *const i32
            50..51 '1': i32
            62..63 '1': i32
            62..63 '1': i32
            67..68 't': &mut i32
            67..80 't as *mut i32': *mut i32
            90..91 '2': i32
            90..91 '2': i32
            95..96 't': &mut i32
            95..104 't as &i32': &i32
            114..115 '_': i32
            119..120 't': &mut i32
            119..134 't as *const i32': *const i32
    "]],
    );
}

#[test]
fn return_coerce_unknown() {
    check_infer_with_mismatches(
        r"
        fn foo() -> u32 {
            return unknown;
        }
        ",
        expect![[r"
            16..39 '{     ...own; }': u32
            22..36 'return unknown': !
            29..36 'unknown': u32
        "]],
    );
}

#[test]
fn coerce_autoderef() {
    check_infer_with_mismatches(
        r"
        struct Foo;
        fn takes_ref_foo(x: &Foo) {}
        fn test() {
            takes_ref_foo(&Foo);
            takes_ref_foo(&&Foo);
            takes_ref_foo(&&&Foo);
        }
        ",
        expect![[r"
            29..30 'x': &Foo
            38..40 '{}': ()
            51..132 '{     ...oo); }': ()
            57..70 'takes_ref_foo': fn takes_ref_foo(&Foo)
            57..76 'takes_...(&Foo)': ()
            71..75 '&Foo': &Foo
            72..75 'Foo': Foo
            82..95 'takes_ref_foo': fn takes_ref_foo(&Foo)
            82..102 'takes_...&&Foo)': ()
            96..101 '&&Foo': &&Foo
            97..101 '&Foo': &Foo
            98..101 'Foo': Foo
            108..121 'takes_ref_foo': fn takes_ref_foo(&Foo)
            108..129 'takes_...&&Foo)': ()
            122..128 '&&&Foo': &&&Foo
            123..128 '&&Foo': &&Foo
            124..128 '&Foo': &Foo
            125..128 'Foo': Foo
        "]],
    );
}

#[test]
fn coerce_autoderef_generic() {
    check_infer_with_mismatches(
        r#"
struct Foo;
fn takes_ref<T>(x: &T) -> T { *x }
fn test() {
    takes_ref(&Foo);
    takes_ref(&&Foo);
    takes_ref(&&&Foo);
}
"#,
        expect![[r"
            28..29 'x': &T
            40..46 '{ *x }': T
            42..44 '*x': T
            43..44 'x': &T
            57..126 '{     ...oo); }': ()
            63..72 'takes_ref': fn takes_ref<Foo>(&Foo) -> Foo
            63..78 'takes_ref(&Foo)': Foo
            73..77 '&Foo': &Foo
            74..77 'Foo': Foo
            84..93 'takes_ref': fn takes_ref<&Foo>(&&Foo) -> &Foo
            84..100 'takes_...&&Foo)': &Foo
            94..99 '&&Foo': &&Foo
            95..99 '&Foo': &Foo
            96..99 'Foo': Foo
            106..115 'takes_ref': fn takes_ref<&&Foo>(&&&Foo) -> &&Foo
            106..123 'takes_...&&Foo)': &&Foo
            116..122 '&&&Foo': &&&Foo
            117..122 '&&Foo': &&Foo
            118..122 '&Foo': &Foo
            119..122 'Foo': Foo
        "]],
    );
}

#[test]
fn coerce_autoderef_block() {
    check_infer_with_mismatches(
        r#"
//- minicore: deref
struct String {}
impl core::ops::Deref for String { type Target = str; }
fn takes_ref_str(x: &str) {}
fn returns_string() -> String { loop {} }
fn test() {
    takes_ref_str(&{ returns_string() });
}
"#,
        expect![[r#"
            90..91 'x': &str
            99..101 '{}': ()
            132..143 '{ loop {} }': String
            134..141 'loop {}': !
            139..141 '{}': ()
            154..199 '{     ... }); }': ()
            160..173 'takes_ref_str': fn takes_ref_str(&str)
            160..196 'takes_...g() })': ()
            174..195 '&{ ret...ng() }': &String
            175..195 '{ retu...ng() }': String
            177..191 'returns_string': fn returns_string() -> String
            177..193 'return...ring()': String
        "#]],
    );
}

#[test]
fn closure_return_coerce() {
    check_infer_with_mismatches(
        r"
        fn foo() {
            let x = || {
                if true {
                    return &1u32;
                }
                &&1u32
            };
        }
        ",
        expect![[r"
            9..105 '{     ...  }; }': ()
            19..20 'x': || -> &u32
            23..102 '|| {  ...     }': || -> &u32
            26..102 '{     ...     }': &u32
            36..81 'if tru...     }': ()
            39..43 'true': bool
            44..81 '{     ...     }': ()
            58..70 'return &1u32': !
            65..70 '&1u32': &u32
            66..70 '1u32': u32
            90..96 '&&1u32': &&u32
            91..96 '&1u32': &u32
            92..96 '1u32': u32
        "]],
    );
}

#[test]
fn coerce_fn_item_to_fn_ptr() {
    check_infer_with_mismatches(
        r"
        fn foo(x: u32) -> isize { 1 }
        fn test() {
            let f: fn(u32) -> isize = foo;
        }
        ",
        expect![[r"
            7..8 'x': u32
            24..29 '{ 1 }': isize
            26..27 '1': isize
            40..78 '{     ...foo; }': ()
            50..51 'f': fn(u32) -> isize
            72..75 'foo': fn foo(u32) -> isize
        "]],
    );
}

#[test]
fn coerce_fn_items_in_match_arms() {
    cov_mark::check!(coerce_fn_reification);

    check_infer_with_mismatches(
        r"
        fn foo1(x: u32) -> isize { 1 }
        fn foo2(x: u32) -> isize { 2 }
        fn foo3(x: u32) -> isize { 3 }
        fn test() {
            let x = match 1 {
                1 => foo1,
                2 => foo2,
                _ => foo3,
            };
        }
        ",
        expect![[r"
            8..9 'x': u32
            25..30 '{ 1 }': isize
            27..28 '1': isize
            39..40 'x': u32
            56..61 '{ 2 }': isize
            58..59 '2': isize
            70..71 'x': u32
            87..92 '{ 3 }': isize
            89..90 '3': isize
            103..192 '{     ...  }; }': ()
            113..114 'x': fn(u32) -> isize
            117..189 'match ...     }': fn(u32) -> isize
            123..124 '1': i32
            135..136 '1': i32
            135..136 '1': i32
            140..144 'foo1': fn foo1(u32) -> isize
            154..155 '2': i32
            154..155 '2': i32
            159..163 'foo2': fn foo2(u32) -> isize
            173..174 '_': i32
            178..182 'foo3': fn foo3(u32) -> isize
        "]],
    );
}

#[test]
fn coerce_closure_to_fn_ptr() {
    check_infer_with_mismatches(
        r"
        fn test() {
            let f: fn(u32) -> isize = |x| { 1 };
        }
        ",
        expect![[r"
            10..54 '{     ...1 }; }': ()
            20..21 'f': fn(u32) -> isize
            42..51 '|x| { 1 }': |u32| -> isize
            43..44 'x': u32
            46..51 '{ 1 }': isize
            48..49 '1': isize
        "]],
    );
}

#[test]
fn coerce_placeholder_ref() {
    // placeholders should unify, even behind references
    check_infer_with_mismatches(
        r"
        struct S<T> { t: T }
        impl<TT> S<TT> {
            fn get(&self) -> &TT {
                &self.t
            }
        }
        ",
        expect![[r"
            50..54 'self': &S<TT>
            63..86 '{     ...     }': &TT
            73..80 '&self.t': &TT
            74..78 'self': &S<TT>
            74..80 'self.t': TT
        "]],
    );
}

#[test]
fn coerce_unsize_array() {
    check_infer_with_mismatches(
        r#"
//- minicore: coerce_unsized
fn test() {
    let f: &[usize] = &[1, 2, 3];
}
        "#,
        expect![[r#"
            10..47 '{     ... 3]; }': ()
            20..21 'f': &[usize]
            34..44 '&[1, 2, 3]': &[usize; 3]
            35..44 '[1, 2, 3]': [usize; 3]
            36..37 '1': usize
            39..40 '2': usize
            42..43 '3': usize
        "#]],
    );
}

#[test]
fn coerce_unsize_trait_object_simple() {
    check_infer_with_mismatches(
        r#"
//- minicore: coerce_unsized
trait Foo<T, U> {}
trait Bar<U, T, X>: Foo<T, U> {}
trait Baz<T, X>: Bar<usize, T, X> {}

struct S<T, X>;
impl<T, X> Foo<T, usize> for S<T, X> {}
impl<T, X> Bar<usize, T, X> for S<T, X> {}
impl<T, X> Baz<T, X> for S<T, X> {}

fn test() {
    let obj: &dyn Baz<i8, i16> = &S;
    let obj: &dyn Bar<_, i8, i16> = &S;
    let obj: &dyn Foo<i8, _> = &S;
}
"#,
        expect![[r#"
            236..351 '{     ... &S; }': ()
            246..249 'obj': &dyn Baz<i8, i16>
            271..273 '&S': &S<i8, i16>
            272..273 'S': S<i8, i16>
            283..286 'obj': &dyn Bar<usize, i8, i16>
            311..313 '&S': &S<i8, i16>
            312..313 'S': S<i8, i16>
            323..326 'obj': &dyn Foo<i8, usize>
            346..348 '&S': &S<i8, {unknown}>
            347..348 'S': S<i8, {unknown}>
        "#]],
    );
}

#[test]
fn coerce_unsize_trait_object_to_trait_object() {
    // FIXME: The rust reference says this should be possible, but rustc doesn't
    // implement it. We used to support it, but Chalk doesn't. Here's the
    // correct expect:
    //
    //     424..609 '{     ...bj2; }': ()
    //     434..437 'obj': &dyn Baz<i8, i16>
    //     459..461 '&S': &S<i8, i16>
    //     460..461 'S': S<i8, i16>
    //     471..474 'obj': &dyn Bar<usize, i8, i16>
    //     496..499 'obj': &dyn Baz<i8, i16>
    //     509..512 'obj': &dyn Foo<i8, usize>
    //     531..534 'obj': &dyn Bar<usize, i8, i16>
    //     544..548 'obj2': &dyn Baz<i8, i16>
    //     570..572 '&S': &S<i8, i16>
    //     571..572 'S': S<i8, i16>
    //     582..583 '_': &dyn Foo<i8, usize>
    //     602..606 'obj2': &dyn Baz<i8, i16>
    check_infer_with_mismatches(
        r#"
//- minicore: coerce_unsized
trait Foo<T, U> {}
trait Bar<U, T, X>: Foo<T, U> {}
trait Baz<T, X>: Bar<usize, T, X> {}

struct S<T, X>;
impl<T, X> Foo<T, usize> for S<T, X> {}
impl<T, X> Bar<usize, T, X> for S<T, X> {}
impl<T, X> Baz<T, X> for S<T, X> {}

fn test() {
    let obj: &dyn Baz<i8, i16> = &S;
    let obj: &dyn Bar<_, _, _> = obj;
    let obj: &dyn Foo<_, _> = obj;
    let obj2: &dyn Baz<i8, i16> = &S;
    let _: &dyn Foo<_, _> = obj2;
}
"#,
        expect![[r#"
            236..421 '{     ...bj2; }': ()
            246..249 'obj': &dyn Baz<i8, i16>
            271..273 '&S': &S<i8, i16>
            272..273 'S': S<i8, i16>
            283..286 'obj': &dyn Bar<{unknown}, {unknown}, {unknown}>
            308..311 'obj': &dyn Baz<i8, i16>
            321..324 'obj': &dyn Foo<{unknown}, {unknown}>
            343..346 'obj': &dyn Bar<{unknown}, {unknown}, {unknown}>
            356..360 'obj2': &dyn Baz<i8, i16>
            382..384 '&S': &S<i8, i16>
            383..384 'S': S<i8, i16>
            394..395 '_': &dyn Foo<{unknown}, {unknown}>
            414..418 'obj2': &dyn Baz<i8, i16>
            308..311: expected &dyn Bar<{unknown}, {unknown}, {unknown}>, got &dyn Baz<i8, i16>
            343..346: expected &dyn Foo<{unknown}, {unknown}>, got &dyn Bar<{unknown}, {unknown}, {unknown}>
            414..418: expected &dyn Foo<{unknown}, {unknown}>, got &dyn Baz<i8, i16>
        "#]],
    );
}

#[test]
fn coerce_unsize_super_trait_cycle() {
    check_infer_with_mismatches(
        r#"
//- minicore: coerce_unsized
trait A {}
trait B: C + A {}
trait C: B {}
trait D: C

struct S;
impl A for S {}
impl B for S {}
impl C for S {}
impl D for S {}

fn test() {
    let obj: &dyn D = &S;
    let obj: &dyn A = &S;
}
"#,
        expect![[r#"
            140..195 '{     ... &S; }': ()
            150..153 'obj': &dyn D
            164..166 '&S': &S
            165..166 'S': S
            176..179 'obj': &dyn A
            190..192 '&S': &S
            191..192 'S': S
        "#]],
    );
}

#[test]
fn coerce_unsize_generic() {
    // FIXME: fix the type mismatches here
    check_infer_with_mismatches(
        r#"
//- minicore: coerce_unsized
struct Foo<T> { t: T };
struct Bar<T>(Foo<T>);

fn test() {
    let _: &Foo<[usize]> = &Foo { t: [1, 2, 3] };
    let _: &Bar<[usize]> = &Bar(Foo { t: [1, 2, 3] });
}
"#,
        expect![[r#"
            58..166 '{     ... }); }': ()
            68..69 '_': &Foo<[usize]>
            87..108 '&Foo {..., 3] }': &Foo<[usize]>
            88..108 'Foo { ..., 3] }': Foo<[usize]>
            97..106 '[1, 2, 3]': [usize; 3]
            98..99 '1': usize
            101..102 '2': usize
            104..105 '3': usize
            118..119 '_': &Bar<[usize]>
            137..163 '&Bar(F... 3] })': &Bar<[i32; 3]>
            138..141 'Bar': Bar<[i32; 3]>(Foo<[i32; 3]>) -> Bar<[i32; 3]>
            138..163 'Bar(Fo... 3] })': Bar<[i32; 3]>
            142..162 'Foo { ..., 3] }': Foo<[i32; 3]>
            151..160 '[1, 2, 3]': [i32; 3]
            152..153 '1': i32
            155..156 '2': i32
            158..159 '3': i32
            97..106: expected [usize], got [usize; 3]
            137..163: expected &Bar<[usize]>, got &Bar<[i32; 3]>
        "#]],
    );
}

#[test]
fn coerce_unsize_apit() {
    // FIXME: #8984
    check_infer_with_mismatches(
        r#"
//- minicore: coerce_unsized
trait Foo {}

fn test(f: impl Foo) {
    let _: &dyn Foo = &f;
}
        "#,
        expect![[r#"
            22..23 'f': impl Foo
            35..64 '{     ... &f; }': ()
            45..46 '_': &dyn Foo
            59..61 '&f': &impl Foo
            60..61 'f': impl Foo
            59..61: expected &dyn Foo, got &impl Foo
        "#]],
    );
}

#[test]
fn infer_two_closures_lub() {
    check_types(
        r#"
fn foo(c: i32) {
    let add = |a: i32, b: i32| a + b;
    let sub = |a, b| a - b;
            //^^^^^^^^^^^^ |i32, i32| -> i32
    if c > 42 { add } else { sub };
  //^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ fn(i32, i32) -> i32
}
        "#,
    )
}

#[test]
fn infer_match_diverging_branch_1() {
    check_types(
        r#"
enum Result<T> { Ok(T), Err }
fn parse<T>() -> T { loop {} }

fn test() -> i32 {
    let a = match parse() {
        Ok(val) => val,
        Err => return 0,
    };
    a
  //^ i32
}
        "#,
    )
}

#[test]
fn infer_match_diverging_branch_2() {
    // same as 1 except for order of branches
    check_types(
        r#"
enum Result<T> { Ok(T), Err }
fn parse<T>() -> T { loop {} }

fn test() -> i32 {
    let a = match parse() {
        Err => return 0,
        Ok(val) => val,
    };
    a
  //^ i32
}
        "#,
    )
}

#[test]
fn panic_macro() {
    check_no_mismatches(
        r#"
mod panic {
    #[macro_export]
    pub macro panic_2015 {
        () => (
            $crate::panicking::panic()
        ),
    }
}

mod panicking {
    pub fn panic() -> ! { loop {} }
}

#[rustc_builtin_macro = "core_panic"]
macro_rules! panic {
    // Expands to either `$crate::panic::panic_2015` or `$crate::panic::panic_2021`
    // depending on the edition of the caller.
    ($($arg:tt)*) => {
        /* compiler built-in */
    };
}

fn main() {
    panic!()
}
        "#,
    );
}

#[test]
fn coerce_unsize_expected_type() {
    check_no_mismatches(
        r#"
//- minicore: coerce_unsized
fn main() {
    let foo: &[u32] = &[1, 2];
    let foo: &[u32] = match true {
        true => &[1, 2],
        false => &[1, 2, 3],
    };
    let foo: &[u32] = if true {
        &[1, 2]
    } else {
        &[1, 2, 3]
    };
}
        "#,
    );
}
