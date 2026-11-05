//! 変数の置き場所（design.md 5.1）

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::mem;
use std::rc::Rc;

use super::gc;
use super::value::Value;

/// 変数名のための軽いハッシュ（FxHash 相当）。外から来るキーではないので、衝突攻撃への備えは要らない
#[derive(Default)]
pub struct NameHasher(u64);

impl Hasher for NameHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(b)).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }

    fn write_usize(&mut self, n: usize) {
        self.0 = (self.0.rotate_left(5) ^ n as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

pub type NameBuild = BuildHasherDefault<NameHasher>;

type NameMap = HashMap<Rc<str>, Value, NameBuild>;

/// 小さい変数表は線形探索の方がハッシュより速いので、これを超えたらハッシュ表にする
const SMALL_MAX: usize = 12;

/// 変数表の中身。関数のスコープのように変数が少ないうちは並べた配列で持つ
#[derive(Debug)]
enum Vars {
    Small(Vec<(Rc<str>, Value)>),
    Large(NameMap),
}

impl Default for Vars {
    fn default() -> Self {
        Vars::Small(Vec::new())
    }
}

fn same_name(a: &Rc<str>, b: &str) -> bool {
    a.len() == b.len() && **a == *b
}

impl Vars {
    fn get(&self, name: &str) -> Option<&Value> {
        match self {
            Vars::Small(v) => v.iter().find(|(k, _)| same_name(k, name)).map(|(_, v)| v),
            Vars::Large(m) => m.get(name),
        }
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut Value> {
        match self {
            Vars::Small(v) => v
                .iter_mut()
                .find(|(k, _)| same_name(k, name))
                .map(|(_, v)| v),
            Vars::Large(m) => m.get_mut(name),
        }
    }

    fn insert(&mut self, name: Rc<str>, value: Value) {
        if let Some(slot) = self.get_mut(&name) {
            *slot = value;
            return;
        }
        match self {
            Vars::Small(v) if v.len() < SMALL_MAX => v.push((name, value)),
            Vars::Small(v) => {
                let mut map: NameMap = v.drain(..).collect();
                map.insert(name, value);
                *self = Vars::Large(map);
            }
            Vars::Large(m) => {
                m.insert(name, value);
            }
        }
    }

    fn for_each_value(&self, f: &mut dyn FnMut(&Value)) {
        match self {
            Vars::Small(v) => v.iter().for_each(|(_, v)| f(v)),
            Vars::Large(m) => m.values().for_each(f),
        }
    }
}

/// スコープ 1 つ分の変数表。外側のスコープへの参照を持ち、名前は内側から順に探す。
/// トップレベルのスコープ（ファイルの変数）は `parent` を持たない
#[derive(Debug, Default)]
pub struct Env {
    vars: RefCell<Vars>,
    parent: Option<Rc<Env>>,
}

impl Env {
    pub fn new(parent: Option<Rc<Env>>) -> Rc<Self> {
        let env = Rc::new(Self {
            vars: RefCell::default(),
            parent,
        });
        gc::register_env(&env);
        env
    }

    /// 循環回収器のために、持っている配列・マップ・関数値・外側のスコープのアドレスを順に渡す
    pub fn for_each_child(&self, f: &mut dyn FnMut(usize)) {
        if let Ok(vars) = self.vars.try_borrow() {
            vars.for_each_value(&mut |v| gc::value_child(v, f));
        }
        if let Some(parent) = &self.parent {
            f(Rc::as_ptr(parent) as usize);
        }
    }

    /// 変数をすべて捨てる（循環回収器が、ごみになったスコープの循環を壊すために使う）
    pub fn clear(&self) {
        let vars = mem::take(&mut *self.vars.borrow_mut());
        drop(vars);
    }

    /// 内側から順に探す
    pub fn get(&self, name: &str) -> Option<Value> {
        match self.vars.borrow().get(name) {
            Some(value) => Some(value.clone()),
            None => self.parent.as_ref()?.get(name),
        }
    }

    /// このスコープだけを探す（`@u.name` でモジュールの変数を読むときに使う）
    pub fn get_own(&self, name: &str) -> Option<Value> {
        self.vars.borrow().get(name).cloned()
    }

    /// このスコープに新しい変数を作る。同じ名前があれば置き換える
    /// （同じスコープの二重宣言は Resolver が弾く。REPL の再入力は置き換えを許す）
    pub fn declare(&self, name: &str, value: Value) {
        self.vars.borrow_mut().insert(name.into(), value);
    }

    /// `declare` と同じ。名前を `Rc<str>` で持っているときは、文字列の確保を省ける
    pub fn declare_rc(&self, name: &Rc<str>, value: Value) {
        self.vars.borrow_mut().insert(Rc::clone(name), value);
    }

    /// 宣言済みの最も内側の変数を書き換える。見つからなければ false
    pub fn assign(&self, name: &str, value: Value) -> bool {
        if let Some(slot) = self.vars.borrow_mut().get_mut(name) {
            *slot = value;
            return true;
        }
        match &self.parent {
            Some(parent) => parent.assign(name, value),
            None => false,
        }
    }
}
