//! 変数の置き場所（design.md 5.1）

use std::cell::{Cell, RefCell};
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

    fn finish(&self) -> u64 {
        self.0
    }
}

/// 軽いハッシュを使う表（名前や関数の定義のアドレスで引く表用）
pub type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<NameHasher>>;

/// この数までの変数は、ハッシュを使わず順に探す（ブロックのスコープは変数が少ないので、そのほうが速い）
const LINEAR_MAX: usize = 8;

/// 変数の並び。宣言した順に並べ、消さない（置き換えは同じ位置に上書き）ので、位置は変わらない
#[derive(Debug, Default)]
struct Vars {
    items: Vec<(String, Value)>,
    /// 変数が多いスコープ（トップレベルなど）のための、名前 → 位置
    index: Option<FastMap<String, usize>>,
}

impl Vars {
    fn find(&self, name: &str) -> Option<usize> {
        match &self.index {
            Some(index) => index.get(name).copied(),
            None => self.items.iter().position(|(n, _)| n == name),
        }
    }

    fn get(&self, name: &str) -> Option<&Value> {
        self.find(name).map(|i| &self.items[i].1)
    }

    fn insert(&mut self, name: &str, value: Value) {
        if let Some(i) = self.find(name) {
            self.items[i].1 = value;
            return;
        }
        self.items.push((name.to_string(), value));
        let len = self.items.len();
        if let Some(index) = &mut self.index {
            index.insert(name.to_string(), len - 1);
        } else if len > LINEAR_MAX {
            self.index = Some(
                self.items
                    .iter()
                    .enumerate()
                    .map(|(i, (n, _))| (n.clone(), i))
                    .collect(),
            );
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
            vars.items.iter().for_each(|(_, v)| gc::value_child(v, f));
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

    /// トップレベルのスコープの変数を読む。`cache` に前回見つけた位置を覚えておき、
    /// 同じ名前が同じ位置にあれば、探さずに済ませる（位置は変わらない）
    pub fn get_cached(&self, name: &str, cache: &Cell<u32>) -> Option<Value> {
        let vars = self.vars.borrow();
        if let Some((n, value)) = vars.items.get(cache.get() as usize)
            && n == name
        {
            return Some(value.clone());
        }
        let i = vars.find(name)?;
        cache.set(i as u32);
        Some(vars.items[i].1.clone())
    }

    /// `get_cached` の代入版（このスコープだけを探す）。なければ false
    pub fn assign_cached(&self, name: &str, value: Value, cache: &Cell<u32>) -> bool {
        let mut vars = self.vars.borrow_mut();
        let i = match vars.items.get(cache.get() as usize) {
            Some((n, _)) if n == name => cache.get() as usize,
            _ => match vars.find(name) {
                Some(i) => {
                    cache.set(i as u32);
                    i
                }
                None => return false,
            },
        };
        vars.items[i].1 = value;
        true
    }

    /// このスコープに新しい変数を作る。同じ名前があれば置き換える
    /// （同じスコープの二重宣言は Resolver が弾く。REPL の再入力は置き換えを許す）
    pub fn declare(&self, name: &str, value: Value) {
        self.vars.borrow_mut().insert(name, value);
    }

    /// 宣言済みの最も内側の変数を書き換える。見つからなければ false
    pub fn assign(&self, name: &str, value: Value) -> bool {
        {
            let mut vars = self.vars.borrow_mut();
            if let Some(i) = vars.find(name) {
                vars.items[i].1 = value;
                return true;
            }
        }
        match &self.parent {
            Some(parent) => parent.assign(name, value),
            None => false,
        }
    }
}
