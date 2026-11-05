//! 関数の本体のコンパイル

use super::*;

impl Compiler {
    pub(super) fn function_body(&mut self, def: &FuncDef) {
        self.chunk.param_names = def.params.iter().map(|p| intern(&p.name)).collect();
        self.chunk.rest_name = def.rest.as_deref().map(intern);
        for (i, param) in def.params.iter().enumerate() {
            if let Some(default) = &param.default {
                let skip = self.emit(
                    Op::SkipIfGiven {
                        index: i as u32,
                        target: 0,
                    },
                    default.span,
                );
                self.expr(default);
                if self.slots.is_some() {
                    self.emit(Op::StoreLocal(i as u32), default.span);
                } else {
                    let name = self.name(&param.name);
                    self.emit(Op::Declare(name), default.span);
                }
                self.patch_here(skip);
            }
            if let Some(slots) = &mut self.slots {
                slots.pending.retain(|p| p != &param.name);
                slots.scopes[0].push((param.name.clone(), i as u32));
            }
        }
        if def.record {
            // レコードのコンストラクタ（design.md 5.55）: 引数を名前でまとめてタグをつける
            for param in &def.params {
                self.constant(Value::Str(param.name.as_str().into()), def.span);
                self.emit_load(&param.name, def.span);
            }
            self.emit(Op::MakeMap(def.params.len() as u32), def.span);
            let name = self.name(&def.name);
            self.emit(Op::TagRecord(name), def.span);
            self.emit(Op::Return, def.span);
            return;
        }
        self.block(&def.body);
        self.nil(def.span);
        self.emit(Op::Return, def.span);
    }
}
