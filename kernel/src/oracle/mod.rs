//! ORACLE — HALCYON's interpreter, and the persona the shell talks through.

pub mod builtins;
pub mod eval;
pub mod persona;
pub mod reader;
pub mod value;

pub use eval::Interpreter;
pub use value::Value;
