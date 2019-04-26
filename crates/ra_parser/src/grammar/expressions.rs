mod atom;

pub(crate) use self::atom::match_arm_list;
pub(super) use self::atom::{literal, LITERAL_FIRST};
use super::*;

pub(super) enum StmtWithSemi {
    Yes,
    No,
    Optional,
}

const EXPR_FIRST: TokenSet = LHS_FIRST;

pub(super) fn expr(p: &mut Parser) -> BlockLike {
    let r = Restrictions { forbid_structs: false, prefer_stmt: false };
    let mut dollar_lvl = 0;
    expr_bp(p, r, 1, &mut dollar_lvl).1
}

pub(super) fn expr_stmt(p: &mut Parser) -> (Option<CompletedMarker>, BlockLike) {
    let r = Restrictions { forbid_structs: false, prefer_stmt: true };
    let mut dollar_lvl = 0;
    expr_bp(p, r, 1, &mut dollar_lvl)
}

fn expr_no_struct(p: &mut Parser) {
    let r = Restrictions { forbid_structs: true, prefer_stmt: false };
    let mut dollar_lvl = 0;
    expr_bp(p, r, 1, &mut dollar_lvl);
}

// test block
// fn a() {}
// fn b() { let _ = 1; }
// fn c() { 1; 2; }
// fn d() { 1; 2 }
pub(crate) fn block(p: &mut Parser) {
    if !p.at(L_CURLY) {
        p.error("expected a block");
        return;
    }
    let m = p.start();
    p.bump();
    expr_block_contents(p);
    p.expect(R_CURLY);
    m.complete(p, BLOCK);
}

fn is_expr_stmt_attr_allowed(kind: SyntaxKind) -> bool {
    match kind {
        BIN_EXPR | RANGE_EXPR | IF_EXPR => false,
        _ => true,
    }
}

pub(super) fn stmt(p: &mut Parser, with_semi: StmtWithSemi) {
    // test block_items
    // fn a() { fn b() {} }
    let m = p.start();
    // test attr_on_expr_stmt
    // fn foo() {
    //     #[A] foo();
    //     #[B] bar!{}
    //     #[C] #[D] {}
    //     #[D] return ();
    // }
    let has_attrs = p.at(POUND);
    attributes::outer_attributes(p);

    if p.at(LET_KW) {
        let_stmt(p, m, with_semi);
        return;
    }

    let m = match items::maybe_item(p, m, items::ItemFlavor::Mod) {
        Ok(()) => return,
        Err(m) => m,
    };

    let (cm, blocklike) = expr_stmt(p);
    let kind = cm.as_ref().map(|cm| cm.kind()).unwrap_or(ERROR);

    if has_attrs && !is_expr_stmt_attr_allowed(kind) {
        // test_err attr_on_expr_not_allowed
        // fn foo() {
        //    #[A] 1 + 2;
        //    #[B] if true {};
        // }
        p.error(format!("attributes are not allowed on {:?}", kind));
    }

    if p.at(R_CURLY) {
        // test attr_on_last_expr_in_block
        // fn foo() {
        //     { #[A] bar!()? }
        //     #[B] &()
        // }
        if let Some(cm) = cm {
            cm.undo_completion(p).abandon(p);
            m.complete(p, kind);
        } else {
            m.abandon(p);
        }
    } else {
        // test no_semi_after_block
        // fn foo() {
        //     if true {}
        //     loop {}
        //     match () {}
        //     while true {}
        //     for _ in () {}
        //     {}
        //     {}
        //     macro_rules! test {
        //          () => {}
        //     }
        //     test!{}
        // }

        match with_semi {
            StmtWithSemi::Yes => {
                if blocklike.is_block() {
                    p.eat(SEMI);
                } else {
                    p.expect(SEMI);
                }
            }
            StmtWithSemi::No => {}
            StmtWithSemi::Optional => {
                if p.at(SEMI) {
                    p.eat(SEMI);
                }
            }
        }

        m.complete(p, EXPR_STMT);
    }

    // test let_stmt;
    // fn foo() {
    //     let a;
    //     let b: i32;
    //     let c = 92;
    //     let d: i32 = 92;
    // }
    fn let_stmt(p: &mut Parser, m: Marker, with_semi: StmtWithSemi) {
        assert!(p.at(LET_KW));
        p.bump();
        patterns::pattern(p);
        if p.at(COLON) {
            types::ascription(p);
        }
        if p.eat(EQ) {
            expressions::expr(p);
        }

        match with_semi {
            StmtWithSemi::Yes => {
                p.expect(SEMI);
            }
            StmtWithSemi::No => {}
            StmtWithSemi::Optional => {
                if p.at(SEMI) {
                    p.eat(SEMI);
                }
            }
        }
        m.complete(p, LET_STMT);
    }
}

pub(crate) fn expr_block_contents(p: &mut Parser) {
    // This is checked by a validator
    attributes::inner_attributes(p);

    while !p.at(EOF) && !p.at(R_CURLY) {
        // test nocontentexpr
        // fn foo(){
        //     ;;;some_expr();;;;{;;;};;;;Ok(())
        // }
        if p.current() == SEMI {
            p.bump();
            continue;
        }

        stmt(p, StmtWithSemi::Yes)
    }
}

#[derive(Clone, Copy)]
struct Restrictions {
    forbid_structs: bool,
    prefer_stmt: bool,
}

enum Op {
    Simple,
    Composite(SyntaxKind, u8),
}

fn current_op(p: &Parser) -> (u8, Op) {
    if let Some(t) = p.current3() {
        match t {
            (L_ANGLE, L_ANGLE, EQ) => return (1, Op::Composite(SHLEQ, 3)),
            (R_ANGLE, R_ANGLE, EQ) => return (1, Op::Composite(SHREQ, 3)),
            _ => (),
        }
    }

    if let Some(t) = p.current2() {
        match t {
            (PLUS, EQ) => return (1, Op::Composite(PLUSEQ, 2)),
            (MINUS, EQ) => return (1, Op::Composite(MINUSEQ, 2)),
            (STAR, EQ) => return (1, Op::Composite(STAREQ, 2)),
            (PERCENT, EQ) => return (1, Op::Composite(PERCENTEQ, 2)),
            (SLASH, EQ) => return (1, Op::Composite(SLASHEQ, 2)),
            (PIPE, EQ) => return (1, Op::Composite(PIPEEQ, 2)),
            (AMP, EQ) => return (1, Op::Composite(AMPEQ, 2)),
            (CARET, EQ) => return (1, Op::Composite(CARETEQ, 2)),
            (PIPE, PIPE) => return (3, Op::Composite(PIPEPIPE, 2)),
            (AMP, AMP) => return (4, Op::Composite(AMPAMP, 2)),
            (L_ANGLE, EQ) => return (5, Op::Composite(LTEQ, 2)),
            (R_ANGLE, EQ) => return (5, Op::Composite(GTEQ, 2)),
            (L_ANGLE, L_ANGLE) => return (9, Op::Composite(SHL, 2)),
            (R_ANGLE, R_ANGLE) => return (9, Op::Composite(SHR, 2)),
            _ => (),
        }
    }

    let bp = match p.current() {
        EQ => 1,
        DOTDOT | DOTDOTEQ => 2,
        EQEQ | NEQ | L_ANGLE | R_ANGLE => 5,
        PIPE => 6,
        CARET => 7,
        AMP => 8,
        MINUS | PLUS => 10,
        STAR | SLASH | PERCENT => 11,
        _ => 0,
    };
    (bp, Op::Simple)
}

// Parses expression with binding power of at least bp.
fn expr_bp(
    p: &mut Parser,
    r: Restrictions,
    mut bp: u8,
    dollar_lvl: &mut usize,
) -> (Option<CompletedMarker>, BlockLike) {
    // `newly_dollar_open` is a flag indicated that dollar is just closed after lhs, e.g.
    // `$1$ + a`
    // We use this flag to skip handling it.
    let mut newly_dollar_open = if p.at_l_dollar() {
        *dollar_lvl += p.eat_l_dollars();
        true
    } else {
        false
    };

    let mut lhs = match lhs(p, r, dollar_lvl) {
        Some((lhs, blocklike)) => {
            // test stmt_bin_expr_ambiguity
            // fn foo() {
            //     let _ = {1} & 2;
            //     {1} &2;
            // }
            if r.prefer_stmt && blocklike.is_block() {
                return (Some(lhs), BlockLike::Block);
            }
            lhs
        }
        None => return (None, BlockLike::NotBlock),
    };

    loop {
        if *dollar_lvl > 0 && p.at_r_dollar() {
            *dollar_lvl -= p.eat_r_dollars(*dollar_lvl);
            if !newly_dollar_open {
                // We "pump" bp for make it highest priority
                bp = 255;
            }
            newly_dollar_open = false;
        }

        let is_range = p.current() == DOTDOT || p.current() == DOTDOTEQ;
        let (op_bp, op) = current_op(p);
        if op_bp < bp {
            break;
        }
        let m = lhs.precede(p);
        match op {
            Op::Simple => p.bump(),
            Op::Composite(kind, n) => {
                p.bump_compound(kind, n);
            }
        }

        expr_bp(p, r, op_bp + 1, dollar_lvl);
        lhs = m.complete(p, if is_range { RANGE_EXPR } else { BIN_EXPR });
    }
    (Some(lhs), BlockLike::NotBlock)
}

const LHS_FIRST: TokenSet =
    atom::ATOM_EXPR_FIRST.union(token_set![AMP, STAR, EXCL, DOTDOT, DOTDOTEQ, MINUS]);

fn lhs(
    p: &mut Parser,
    r: Restrictions,
    dollar_lvl: &mut usize,
) -> Option<(CompletedMarker, BlockLike)> {
    let m;
    let kind = match p.current() {
        // test ref_expr
        // fn foo() {
        //     let _ = &1;
        //     let _ = &mut &f();
        // }
        AMP => {
            m = p.start();
            p.bump();
            p.eat(MUT_KW);
            REF_EXPR
        }
        // test unary_expr
        // fn foo() {
        //     **&1;
        //     !!true;
        //     --1;
        // }
        STAR | EXCL | MINUS => {
            m = p.start();
            p.bump();
            PREFIX_EXPR
        }
        // test full_range_expr
        // fn foo() { xs[..]; }
        DOTDOT | DOTDOTEQ => {
            m = p.start();
            p.bump();
            if p.at_ts(EXPR_FIRST) {
                expr_bp(p, r, 2, dollar_lvl);
            }
            return Some((m.complete(p, RANGE_EXPR), BlockLike::NotBlock));
        }
        _ => {
            let (lhs, blocklike) = atom::atom_expr(p, r)?;
            return Some((
                postfix_expr(p, lhs, !(r.prefer_stmt && blocklike.is_block())),
                blocklike,
            ));
        }
    };
    expr_bp(p, r, 255, dollar_lvl);
    Some((m.complete(p, kind), BlockLike::NotBlock))
}

fn postfix_expr(
    p: &mut Parser,
    mut lhs: CompletedMarker,
    // Calls are disallowed if the type is a block and we prefer statements because the call cannot be disambiguated from a tuple
    // E.g. `while true {break}();` is parsed as
    // `while true {break}; ();`
    mut allow_calls: bool,
) -> CompletedMarker {
    loop {
        lhs = match p.current() {
            // test stmt_postfix_expr_ambiguity
            // fn foo() {
            //     match () {
            //         _ => {}
            //         () => {}
            //         [] => {}
            //     }
            // }
            L_PAREN if allow_calls => call_expr(p, lhs),
            L_BRACK if allow_calls => index_expr(p, lhs),
            DOT if p.nth(1) == IDENT && (p.nth(2) == L_PAREN || p.nth(2) == COLONCOLON) => {
                method_call_expr(p, lhs)
            }
            DOT => field_expr(p, lhs),
            // test postfix_range
            // fn foo() { let x = 1..; }
            DOTDOT | DOTDOTEQ if !EXPR_FIRST.contains(p.nth(1)) => {
                let m = lhs.precede(p);
                p.bump();
                m.complete(p, RANGE_EXPR)
            }
            QUESTION => try_expr(p, lhs),
            AS_KW => cast_expr(p, lhs),
            _ => break,
        };
        allow_calls = true
    }
    lhs
}

// test call_expr
// fn foo() {
//     let _ = f();
//     let _ = f()(1)(1, 2,);
//     let _ = f(<Foo>::func());
//     f(<Foo as Trait>::func());
// }
fn call_expr(p: &mut Parser, lhs: CompletedMarker) -> CompletedMarker {
    assert!(p.at(L_PAREN));
    let m = lhs.precede(p);
    arg_list(p);
    m.complete(p, CALL_EXPR)
}

// test index_expr
// fn foo() {
//     x[1][2];
// }
fn index_expr(p: &mut Parser, lhs: CompletedMarker) -> CompletedMarker {
    assert!(p.at(L_BRACK));
    let m = lhs.precede(p);
    p.bump();
    expr(p);
    p.expect(R_BRACK);
    m.complete(p, INDEX_EXPR)
}

// test method_call_expr
// fn foo() {
//     x.foo();
//     y.bar::<T>(1, 2,);
// }
fn method_call_expr(p: &mut Parser, lhs: CompletedMarker) -> CompletedMarker {
    assert!(p.at(DOT) && p.nth(1) == IDENT && (p.nth(2) == L_PAREN || p.nth(2) == COLONCOLON));
    let m = lhs.precede(p);
    p.bump();
    name_ref(p);
    type_args::opt_type_arg_list(p, true);
    if p.at(L_PAREN) {
        arg_list(p);
    }
    m.complete(p, METHOD_CALL_EXPR)
}

// test field_expr
// fn foo() {
//     x.foo;
//     x.0.bar;
//     x.0();
// }

// test_err bad_tuple_index_expr
// fn foo() {
//     x.0.;
//     x.1i32;
//     x.0x01;
// }
fn field_expr(p: &mut Parser, lhs: CompletedMarker) -> CompletedMarker {
    assert!(p.at(DOT));
    let m = lhs.precede(p);
    p.bump();
    if p.at(IDENT) {
        name_ref(p)
    } else if p.at(INT_NUMBER) {
        p.bump();
    } else if p.at(FLOAT_NUMBER) {
        // FIXME: How to recover and instead parse INT + DOT?
        p.bump();
    } else {
        p.error("expected field name or number")
    }
    m.complete(p, FIELD_EXPR)
}

// test try_expr
// fn foo() {
//     x?;
// }
fn try_expr(p: &mut Parser, lhs: CompletedMarker) -> CompletedMarker {
    assert!(p.at(QUESTION));
    let m = lhs.precede(p);
    p.bump();
    m.complete(p, TRY_EXPR)
}

// test cast_expr
// fn foo() {
//     82 as i32;
//     81 as i8 + 1;
//     79 as i16 - 1;
//     0x36 as u8 <= 0x37;
// }
fn cast_expr(p: &mut Parser, lhs: CompletedMarker) -> CompletedMarker {
    assert!(p.at(AS_KW));
    let m = lhs.precede(p);
    p.bump();
    // Use type_no_bounds(), because cast expressions are not
    // allowed to have bounds.
    types::type_no_bounds(p);
    m.complete(p, CAST_EXPR)
}

fn arg_list(p: &mut Parser) {
    assert!(p.at(L_PAREN));
    let m = p.start();
    p.bump();
    while !p.at(R_PAREN) && !p.at(EOF) {
        if !p.at_ts(EXPR_FIRST) {
            p.error("expected expression");
            break;
        }
        expr(p);
        if !p.at(R_PAREN) && !p.expect(COMMA) {
            break;
        }
    }
    p.eat(R_PAREN);
    m.complete(p, ARG_LIST);
}

// test path_expr
// fn foo() {
//     let _ = a;
//     let _ = a::b;
//     let _ = ::a::<b>;
//     let _ = format!();
// }
fn path_expr(p: &mut Parser, r: Restrictions) -> (CompletedMarker, BlockLike) {
    assert!(paths::is_path_start(p) || p.at(L_ANGLE));
    let m = p.start();
    paths::expr_path(p);
    match p.current() {
        L_CURLY if !r.forbid_structs => {
            named_field_list(p);
            (m.complete(p, STRUCT_LIT), BlockLike::NotBlock)
        }
        EXCL => {
            let block_like = items::macro_call_after_excl(p);
            (m.complete(p, MACRO_CALL), block_like)
        }
        _ => (m.complete(p, PATH_EXPR), BlockLike::NotBlock),
    }
}

// test struct_lit
// fn foo() {
//     S {};
//     S { x, y: 32, };
//     S { x, y: 32, ..Default::default() };
// }
pub(crate) fn named_field_list(p: &mut Parser) {
    assert!(p.at(L_CURLY));
    let m = p.start();
    p.bump();
    while !p.at(EOF) && !p.at(R_CURLY) {
        match p.current() {
            // test struct_literal_field_with_attr
            // fn main() {
            //     S { #[cfg(test)] field: 1 }
            // }
            IDENT | POUND => {
                let m = p.start();
                attributes::outer_attributes(p);
                name_ref(p);
                if p.eat(COLON) {
                    expr(p);
                }
                m.complete(p, NAMED_FIELD);
            }
            DOTDOT => {
                p.bump();
                expr(p);
            }
            L_CURLY => error_block(p, "expected a field"),
            _ => p.err_and_bump("expected identifier"),
        }
        if !p.at(R_CURLY) {
            p.expect(COMMA);
        }
    }
    p.expect(R_CURLY);
    m.complete(p, NAMED_FIELD_LIST);
}
