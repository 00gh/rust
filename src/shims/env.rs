use std::collections::HashMap;

use rustc::ty::layout::{Size, Align};
use rustc_mir::interpret::{Pointer, Memory};
use crate::stacked_borrows::Tag;
use crate::*;

#[derive(Default)]
pub struct EnvVars {
    map: HashMap<Vec<u8>, Pointer<Tag>>,
}

impl EnvVars {
    pub(crate) fn init<'mir, 'tcx>(
        ecx: &mut InterpCx<'mir, 'tcx, Evaluator<'tcx>>,
        communicate: bool,
    ) {
        if communicate {
            for (name, value) in std::env::vars() {
                let value = alloc_env_value(value.as_bytes(), ecx.memory_mut());
                ecx.machine.env_vars.map.insert(name.into_bytes(), value);
            }
        }
    }
}

fn alloc_env_value<'mir, 'tcx>(
    bytes: &[u8],
    memory: &mut Memory<'mir, 'tcx, Evaluator<'tcx>>,
) -> Pointer<Tag> {
    let tcx = {memory.tcx.tcx};
    let length = bytes.len() as u64;
    // `+1` for the null terminator.
    let ptr = memory.allocate(
        Size::from_bytes(length + 1),
        Align::from_bytes(1).unwrap(),
        MiriMemoryKind::Env.into(),
    );
    // We just allocated these, so the write cannot fail.
    let alloc = memory.get_mut(ptr.alloc_id).unwrap();
    alloc.write_bytes(&tcx, ptr, &bytes).unwrap();
    let trailing_zero_ptr = ptr.offset(
        Size::from_bytes(length),
        &tcx,
    ).unwrap();
    alloc.write_bytes(&tcx, trailing_zero_ptr, &[0]).unwrap();
    ptr
}

impl<'mir, 'tcx> EvalContextExt<'mir, 'tcx> for crate::MiriEvalContext<'mir, 'tcx> {}
pub trait EvalContextExt<'mir, 'tcx: 'mir>: crate::MiriEvalContextExt<'mir, 'tcx> {
    fn getenv(
        &mut self,
        name_op: OpTy<'tcx, Tag>,
        dest: PlaceTy<'tcx, Tag>
    ) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();

        let result = {
            let name_ptr = this.read_scalar(name_op)?.not_undef()?;
            let name = this.memory().read_c_str(name_ptr)?;
            match this.machine.env_vars.map.get(name) {
                Some(&var) => Scalar::Ptr(var),
                None => Scalar::ptr_null(&*this.tcx),
            }
        };
        this.write_scalar(result, dest)?;
        Ok(())
    }

    fn setenv(
        &mut self,
        name_op: OpTy<'tcx, Tag>,
        value_op: OpTy<'tcx, Tag>,
        dest: PlaceTy<'tcx, Tag>
    ) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();

        let mut new = None;
        let name_ptr = this.read_scalar(name_op)?.not_undef()?;
        let value_ptr = this.read_scalar(value_op)?.not_undef()?;
        let value = this.memory().read_c_str(value_ptr)?;
        if !this.is_null(name_ptr)? {
            let name = this.memory().read_c_str(name_ptr)?;
            if !name.is_empty() && !name.contains(&b'=') {
                new = Some((name.to_owned(), value.to_owned()));
            }
        }
        if let Some((name, value)) = new {
            let value_copy = alloc_env_value(&value, this.memory_mut());
            if let Some(var) = this.machine.env_vars.map.insert(name.to_owned(), value_copy) {
                this.memory_mut().deallocate(var, None, MiriMemoryKind::Env.into())?;
            }
            this.write_null(dest)?;
        } else {
            this.write_scalar(Scalar::from_int(-1, dest.layout.size), dest)?;
        }
        Ok(())
    }

    fn unsetenv(
        &mut self,
        name_op: OpTy<'tcx, Tag>,
        dest: PlaceTy<'tcx, Tag>
    ) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();

        let mut success = None;
        let name_ptr = this.read_scalar(name_op)?.not_undef()?;
        if !this.is_null(name_ptr)? {
            let name = this.memory().read_c_str(name_ptr)?.to_owned();
            if !name.is_empty() && !name.contains(&b'=') {
                success = Some(this.machine.env_vars.map.remove(&name));
            }
        }
        if let Some(old) = success {
            if let Some(var) = old {
                this.memory_mut().deallocate(var, None, MiriMemoryKind::Env.into())?;
            }
            this.write_null(dest)?;
        } else {
            this.write_scalar(Scalar::from_int(-1, dest.layout.size), dest)?;
        }
        Ok(())
    }
}
