//! ORACLE values and environments.
//!
//! Lists are vectors rather than cons pairs. That costs improper lists, which
//! nothing here needs, and buys a representation with no cycles, no
//! interior mutability, and O(1) length -- worth it for an interpreter that
//! has to be small enough to read in one sitting.

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt::Write;

pub type EvalResult = Result<Value, String>;
pub type BuiltinFn = fn(&mut super::eval::Interpreter, Vec<Value>) -> EvalResult;

#[derive(Clone)]
pub struct Lambda {
    pub name: RefCell<String>,
    pub params: Vec<Rc<str>>,
    /// Name after `&rest`, if the lambda takes a variable argument list.
    pub rest: Option<Rc<str>>,
    pub body: Vec<Value>,
    pub env: Env,
}

#[derive(Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Str(Rc<str>),
    Symbol(Rc<str>),
    List(Rc<Vec<Value>>),
    Builtin(&'static str, BuiltinFn),
    Lambda(Rc<Lambda>),
}

impl Value {
    pub fn symbol(name: &str) -> Value {
        Value::Symbol(Rc::from(name))
    }

    pub fn string(text: &str) -> Value {
        Value::Str(Rc::from(text))
    }

    pub fn list(items: Vec<Value>) -> Value {
        Value::List(Rc::new(items))
    }

    /// Everything except `nil` and `false` is true, as in Scheme.
    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Str(_) => "string",
            Value::Symbol(_) => "symbol",
            Value::List(_) => "list",
            Value::Builtin(..) => "builtin",
            Value::Lambda(_) => "lambda",
        }
    }

    pub fn as_int(&self) -> Result<i64, String> {
        match self {
            Value::Int(value) => Ok(*value),
            other => Err(alloc::format!("expected an int, got {}", other.type_name())),
        }
    }

    pub fn as_str(&self) -> Result<&str, String> {
        match self {
            Value::Str(text) => Ok(text),
            Value::Symbol(name) => Ok(name),
            other => Err(alloc::format!(
                "expected a string, got {}",
                other.type_name()
            )),
        }
    }

    pub fn as_list(&self) -> Result<&[Value], String> {
        match self {
            Value::List(items) => Ok(items),
            Value::Nil => Ok(&[]),
            other => Err(alloc::format!("expected a list, got {}", other.type_name())),
        }
    }

    /// How the REPL echoes a value.
    pub fn display(&self) -> String {
        match self {
            Value::Str(text) => text.to_string(),
            other => other.write_form(),
        }
    }

    /// How a value reads back in as source.
    pub fn write_form(&self) -> String {
        let mut out = String::new();
        self.render(&mut out);
        out
    }

    fn render(&self, out: &mut String) {
        match self {
            Value::Nil => out.push_str("nil"),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Int(value) => {
                let _ = write!(out, "{}", value);
            }
            Value::Str(text) => {
                out.push('"');
                for ch in text.chars() {
                    match ch {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        other => out.push(other),
                    }
                }
                out.push('"');
            }
            Value::Symbol(name) => out.push_str(name),
            Value::List(items) => {
                out.push('(');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(' ');
                    }
                    item.render(out);
                }
                out.push(')');
            }
            Value::Builtin(name, _) => {
                let _ = write!(out, "#<builtin {}>", name);
            }
            Value::Lambda(lambda) => {
                let _ = write!(out, "#<lambda {}>", lambda.name.borrow());
            }
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Symbol(a), Value::Symbol(b)) => a == b,
            (Value::List(a), Value::List(b)) => a.len() == b.len() && a.iter().eq(b.iter()),
            _ => false,
        }
    }
}

// --- environments ---

pub struct Scope {
    values: BTreeMap<Rc<str>, Value>,
    parent: Option<Env>,
}

#[derive(Clone)]
pub struct Env(Rc<RefCell<Scope>>);

impl Env {
    pub fn root() -> Env {
        Env(Rc::new(RefCell::new(Scope {
            values: BTreeMap::new(),
            parent: None,
        })))
    }

    pub fn child(parent: &Env) -> Env {
        Env(Rc::new(RefCell::new(Scope {
            values: BTreeMap::new(),
            parent: Some(parent.clone()),
        })))
    }

    pub fn define(&self, name: Rc<str>, value: Value) {
        self.0.borrow_mut().values.insert(name, value);
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        let scope = self.0.borrow();
        if let Some(value) = scope.values.get(name) {
            return Some(value.clone());
        }
        scope.parent.as_ref()?.get(name)
    }

    /// Assign to an existing binding, walking outwards. Fails if unbound,
    /// which is what distinguishes `set!` from `define`.
    pub fn set(&self, name: &str, value: Value) -> bool {
        let mut scope = self.0.borrow_mut();
        if let Some(slot) = scope.values.get_mut(name) {
            *slot = value;
            return true;
        }
        match scope.parent.clone() {
            Some(parent) => {
                drop(scope);
                parent.set(name, value)
            }
            None => false,
        }
    }

    /// Names bound in this scope, for tab completion and `(env)`.
    pub fn names(&self) -> Vec<String> {
        let mut names = Vec::new();
        let scope = self.0.borrow();
        for key in scope.values.keys() {
            names.push(key.to_string());
        }
        if let Some(parent) = &scope.parent {
            names.extend(parent.names());
        }
        names.sort();
        names.dedup();
        names
    }
}
