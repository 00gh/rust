use super::*;

pub(super) fn is_path_start(p: &Parser) -> bool {
    match p.current() {
        IDENT | SELF_KW | SUPER_KW | COLONCOLON => true,
        _ => false,
    }
}

pub(super) fn use_path(p: &mut Parser) {
    path(p, Mode::Use)
}

pub(super) fn type_path(p: &mut Parser) {
    path(p, Mode::Type)
}

pub(super) fn expr_path(p: &mut Parser) {
    path(p, Mode::Expr)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Mode {
    Use,
    Type,
    Expr,
}

fn path(p: &mut Parser, mode: Mode) {
    if !is_path_start(p) {
        return;
    }
    let path = p.start();
    path_segment(p, mode, true);
    let mut qual = path.complete(p, PATH);
    loop {
        let use_tree = match p.nth(1) {
            STAR | L_CURLY => true,
            _ => false,
        };
        if p.at(COLONCOLON) && !use_tree {
            let path = qual.precede(p);
            p.bump();
            path_segment(p, mode, false);
            let path = path.complete(p, PATH);
            qual = path;
        } else {
            break;
        }
    }
}

fn path_segment(p: &mut Parser, mode: Mode, first: bool) {
    let segment = p.start();
    if first {
        p.eat(COLONCOLON);
    }
    match p.current() {
        IDENT => {
            name_ref(p);
            path_generic_args(p, mode);
        }
        SELF_KW | SUPER_KW => p.bump(),
        _ => {
            p.err_and_bump("expected identifier");
        }
    };
    segment.complete(p, PATH_SEGMENT);
}

fn path_generic_args(p: &mut Parser, mode: Mode) {
    match mode {
        Mode::Use => return,
        Mode::Type => {
            // test path_fn_trait_args
            // type F = Box<Fn(x: i32) -> ()>;
            if p.at(L_PAREN) {
                params::param_list_opt_patterns(p);
                fn_ret_type(p);
            } else {
                type_args::type_arg_list(p, false)
            }
        },
        Mode::Expr => type_args::type_arg_list(p, true),
    }
}
