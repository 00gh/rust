use rustc::front;
use rustc::middle::ty;
use rustc_mir::mir_map::MirMap;
use rustc_mir::repr::{self as mir, Mir};
use syntax::attr::AttrMetaMethods;

#[derive(Clone, Debug)]
enum Value {
    Uninit,
    Bool(bool),
    Int(i64),
}

struct Interpreter<'tcx> {
    mir: &'tcx Mir<'tcx>,
    var_vals: Vec<Value>,
    temp_vals: Vec<Value>,
    result: Value,
}

impl<'tcx> Interpreter<'tcx> {
    fn new(mir: &'tcx Mir<'tcx>) -> Self {
        Interpreter {
            mir: mir,
            var_vals: vec![Value::Uninit; mir.var_decls.len()],
            temp_vals: vec![Value::Uninit; mir.temp_decls.len()],
            result: Value::Uninit,
        }
    }

    fn run(&mut self) {
        let start_block = self.mir.basic_block_data(mir::START_BLOCK);

        for stmt in &start_block.statements {
            use rustc_mir::repr::Lvalue::*;
            use rustc_mir::repr::StatementKind::*;

            println!("  {:?}", stmt);
            match stmt.kind {
                Assign(ref lv, ref rv) => {
                    let val = self.eval_rvalue(rv);

                    let spot = match *lv {
                        Var(i) => &mut self.var_vals[i as usize],
                        Temp(i) => &mut self.temp_vals[i as usize],
                        ReturnPointer => &mut self.result,
                        _ => unimplemented!(),
                    };

                    *spot = val;
                }
                Drop(_kind, ref _lv) => { /* TODO */ },
            }
        }

        println!("  {:?}", start_block.terminator);
        println!("=> {:?}", self.result);
    }

    fn eval_rvalue(&mut self, rvalue: &mir::Rvalue) -> Value {
        use rustc_mir::repr::Rvalue::*;
        use rustc_mir::repr::BinOp::*;

        match *rvalue {
            Use(ref operand) => self.eval_operand(operand),
            BinaryOp(bin_op, ref left, ref right) => {
                match (self.eval_operand(left), self.eval_operand(right)) {
                    (Value::Int(l), Value::Int(r)) => match bin_op {
                        Add => Value::Int(l + r),
                        Sub => Value::Int(l - r),
                        Mul => Value::Int(l * r),
                        Div => Value::Int(l / r),
                        Rem => Value::Int(l % r),
                        BitXor => Value::Int(l ^ r),
                        BitAnd => Value::Int(l & r),
                        BitOr => Value::Int(l | r),
                        Shl => Value::Int(l << r),
                        Shr => Value::Int(l >> r),
                        Eq => Value::Bool(l == r),
                        Lt => Value::Bool(l < r),
                        Le => Value::Bool(l <= r),
                        Ne => Value::Bool(l != r),
                        Ge => Value::Bool(l >= r),
                        Gt => Value::Bool(l > r),
                    },
                    _ => unimplemented!(),
                }
            }
            _ => unimplemented!(),
        }
    }

    fn eval_operand(&mut self, op: &mir::Operand) -> Value {
        use rustc::middle::const_eval::ConstVal::*;
        use rustc_mir::repr::Lvalue::*;
        use rustc_mir::repr::Operand::*;

        match *op {
            Consume(Var(i)) => self.var_vals[i as usize].clone(),
            Consume(Temp(i)) => self.temp_vals[i as usize].clone(),
            Constant(ref constant) => {
                match constant.literal {
                    mir::Literal::Value { value: Int(n) } => Value::Int(n),
                    _ => unimplemented!(),
                }
            }
            _ => unimplemented!(),
        }
    }
}

pub fn interpret_start_points<'tcx>(tcx: &ty::ctxt<'tcx>, mir_map: &MirMap<'tcx>) {
    for (&id, mir) in mir_map {
        for attr in tcx.map.attrs(id) {
            if attr.check_name("miri_run") {
                let item = match tcx.map.get(id) {
                    front::map::NodeItem(item) => item,
                    _ => panic!(),
                };
                println!("Interpreting: {}", item.name);
                let mut interpreter = Interpreter::new(mir);
                interpreter.run();
            }
        }
    }
}
