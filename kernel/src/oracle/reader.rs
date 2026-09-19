//! The reader: source text to ORACLE values.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::value::Value;

pub struct Reader<'a> {
    source: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            position: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.position).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.position += 1;
        Some(byte)
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(byte) if byte.is_ascii_whitespace() => {
                    self.position += 1;
                }
                Some(b';') => {
                    while let Some(byte) = self.peek() {
                        if byte == b'\n' {
                            break;
                        }
                        self.position += 1;
                    }
                }
                _ => return,
            }
        }
    }

    /// Read one form. `Ok(None)` means the input is exhausted.
    pub fn read(&mut self) -> Result<Option<Value>, String> {
        self.skip_trivia();
        let Some(byte) = self.peek() else {
            return Ok(None);
        };

        match byte {
            b'(' | b'[' => {
                let opener = byte;
                self.position += 1;
                let closer = if opener == b'(' { b')' } else { b']' };
                let mut items = Vec::new();
                loop {
                    self.skip_trivia();
                    match self.peek() {
                        None => return Err("unclosed list".to_string()),
                        Some(found) if found == closer => {
                            self.position += 1;
                            break;
                        }
                        Some(b')') | Some(b']') => {
                            return Err("mismatched closing bracket".to_string())
                        }
                        _ => match self.read()? {
                            Some(value) => items.push(value),
                            None => return Err("unclosed list".to_string()),
                        },
                    }
                }
                Ok(Some(Value::list(items)))
            }
            b')' | b']' => Err("unexpected closing bracket".to_string()),
            b'\'' => {
                self.position += 1;
                match self.read()? {
                    Some(value) => Ok(Some(Value::list(alloc::vec![
                        Value::symbol("quote"),
                        value
                    ]))),
                    None => Err("nothing to quote".to_string()),
                }
            }
            b'"' => {
                self.position += 1;
                let mut text = String::new();
                loop {
                    match self.bump() {
                        None => return Err("unterminated string".to_string()),
                        Some(b'"') => break,
                        Some(b'\\') => match self.bump() {
                            Some(b'n') => text.push('\n'),
                            Some(b't') => text.push('\t'),
                            Some(b'\\') => text.push('\\'),
                            Some(b'"') => text.push('"'),
                            Some(other) => text.push(other as char),
                            None => return Err("unterminated escape".to_string()),
                        },
                        Some(other) => text.push(other as char),
                    }
                }
                Ok(Some(Value::Str(Rc::from(text.as_str()))))
            }
            _ => {
                let start = self.position;
                while let Some(byte) = self.peek() {
                    if byte.is_ascii_whitespace()
                        || byte == b'('
                        || byte == b')'
                        || byte == b'['
                        || byte == b']'
                        || byte == b';'
                    {
                        break;
                    }
                    self.position += 1;
                }
                let token = core::str::from_utf8(&self.source[start..self.position])
                    .map_err(|_| "invalid utf-8 in token".to_string())?;
                Ok(Some(atom(token)))
            }
        }
    }

    /// Read every form in the input.
    pub fn read_all(&mut self) -> Result<Vec<Value>, String> {
        let mut forms = Vec::new();
        while let Some(value) = self.read()? {
            forms.push(value);
        }
        Ok(forms)
    }
}

fn atom(token: &str) -> Value {
    match token {
        "nil" => return Value::Nil,
        "true" | "#t" => return Value::Bool(true),
        "false" | "#f" => return Value::Bool(false),
        _ => {}
    }

    // Integers, including 0x-prefixed and negative.
    if let Some(hex) = token.strip_prefix("0x").or_else(|| token.strip_prefix("0X")) {
        if !hex.is_empty() {
            if let Ok(value) = i64::from_str_radix(hex, 16) {
                return Value::Int(value);
            }
        }
    }
    if let Ok(value) = token.parse::<i64>() {
        return Value::Int(value);
    }

    Value::Symbol(Rc::from(token))
}

/// Parse a single expression, rejecting trailing input.
pub fn parse_one(source: &str) -> Result<Value, String> {
    let mut reader = Reader::new(source);
    let value = reader
        .read()?
        .ok_or_else(|| "empty expression".to_string())?;
    reader.skip_trivia();
    if reader.position < reader.source.len() {
        return Err(format!(
            "unexpected text after expression: {:?}",
            core::str::from_utf8(&reader.source[reader.position..]).unwrap_or("?")
        ));
    }
    Ok(value)
}
