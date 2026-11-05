//! 命令の振り分け。簡単な命令はここで済ませ、長い処理は種類ごとのファイル（`control`・`collection`・`call`・
//! `for_loop`・`unwind`・`coroutine`）の関数に任せる

use super::*;

impl Interpreter<'_> {
    /// 1 命令を実行する。`run_vm_exit` の速い経路で済まない命令がここに来る
    pub(super) fn step(
        &mut self,
        op: &Op,
        span: Span,
        chunk: &Rc<Chunk>,
        ip: &mut usize,
        base: usize,
        target: usize,
    ) -> RResult<Step> {
        match op {
            Op::Const(i) => self.stack.push(chunk.consts[*i as usize].clone()),
            Op::LoadLocal(slot) => {
                let value = self.stack[base + *slot as usize].clone();
                self.stack.push(value);
            }
            Op::StoreLocal(slot) => {
                let value = self.pop();
                self.stack[base + *slot as usize] = value;
            }
            Op::LoadVar(n) => self.load_var(&chunk.names[*n as usize], span)?,
            Op::Declare(n) => {
                let value = self.pop();
                self.env.declare(&chunk.names[*n as usize], value);
            }
            Op::AssignVar(n) => self.assign_var(&chunk.names[*n as usize], span)?,
            Op::Pop => {
                self.pop();
            }
            Op::PopCheckError => self.pop_check_error(span)?,
            Op::PushScope => self.push_scope(),
            Op::PopScope => self.pop_scope(),
            Op::Jump(t) => *ip = *t as usize,
            Op::JumpIfFalseCond(t) => self.jump_if_false_cond(*t, ip, span)?,
            Op::LoopTick => gc::maybe_collect(),
            Op::LogicOperand(op) => self.logic_operand(*op, span)?,
            Op::ShortCircuit(op, t) => self.short_circuit(*op, *t, ip),
            Op::Binary(op) => {
                let r = self.pop();
                let l = self.pop();
                self.stack.push(binary_values(*op, &l, &r, span)?);
            }
            Op::Unary(op) => self.unary(*op, span)?,
            Op::MakeArray(n) => {
                let items = self.split_top(*n as usize);
                self.stack.push(gc::new_array(items));
            }
            Op::ArrayNew => self.stack.push(gc::new_array(Vec::new())),
            Op::ArrayAppend => self.array_append(),
            Op::ArrayExtend => self.array_extend(span)?,
            Op::CheckKey => self.check_key(span)?,
            Op::MakeMap(n) => self.make_map(*n as usize),
            Op::TagRecord(name) => self.tag_record(&chunk.names[*name as usize]),
            Op::ToDisplay => {
                let value = self.pop();
                self.stack.push(Value::Str(value.to_string().into()));
            }
            Op::Concat(n) => self.concat(*n as usize),
            Op::IndexGet { target, key } => {
                let k = self.pop();
                let container = self.pop();
                self.stack.push(index_get(&container, &k, *target, *key)?);
            }
            Op::IndexSet { target, key } => {
                let value = self.pop();
                let k = self.pop();
                let container = self.pop();
                index_set(&container, &k, value, *target, *key)?;
            }
            Op::RejectModule => self.reject_module_top(span)?,
            Op::Member { name, target } => {
                let base = self.pop();
                let value = self.member(base, &chunk.names[*name as usize], *target, span)?;
                self.stack.push(value);
            }
            Op::FuncRef(n) => {
                let value = self.func_value(&chunk.names[*n as usize], span)?;
                self.stack.push(value);
            }
            Op::MakeClosure(i) => self.make_closure(chunk, *i as usize),
            Op::Await => return self.await_op(ip, span, target),
            Op::Try => return self.try_op(span, target),
            Op::Return => {
                let value = self.pop();
                return self.do_return(value, target);
            }
            Op::Defer => self.defer_op(span),
            Op::Call { name, argc } => {
                return self.call_op(&chunk.names[*name as usize], *argc as usize, span, *ip);
            }
            Op::CallSpread { name } => {
                let args = self.pop_spread_args();
                return self.call_named(&chunk.names[*name as usize], args, span, *ip);
            }
            Op::CallValue { argc, callee } => {
                return self.call_value_op(*argc as usize, *callee, span, *ip);
            }
            Op::CallValueSpread { callee } => {
                let args = self.pop_spread_args();
                let f = self.pop();
                return self.call_function_op(&f, args, span, *callee, *ip);
            }
            Op::MemberCall { name, argc, target } => {
                let args = self.split_top(*argc as usize);
                let base = self.pop();
                let name = &chunk.names[*name as usize];
                return self.member_call(base, name, args, span, *target, *ip);
            }
            Op::MemberCallSpread { name, target } => {
                let args = self.pop_spread_args();
                let base = self.pop();
                let name = &chunk.names[*name as usize];
                return self.member_call(base, name, args, span, *target, *ip);
            }
            Op::SkipIfGiven { index, target } => {
                let given = self.vm_frames.last().map_or(0, |f| f.given);
                if (*index as usize) < given {
                    *ip = *target as usize;
                }
            }
            Op::Destructure { pattern, declare } => {
                let value = self.pop();
                let pattern: &Pattern = &chunk.patterns[*pattern as usize];
                self.destructure(pattern, &value, *declare, span)?;
            }
            Op::MatchPat { pattern, fail } => {
                self.match_pat(&chunk.matches[*pattern as usize], *fail, ip)
            }
            Op::MatchFail => return Err(self.match_fail(span)),
            Op::ForPrep => self.for_prep(span)?,
            Op::ForNextLocal { end, slot } => self.for_next_local(*end, *slot, ip, base),
            Op::ForNext { end, var } => self.for_next(*end, &chunk.names[*var as usize], ip),
        }
        Ok(Step::Next)
    }
}
