// SPDX-License-Identifier: GPL-3.0-or-later
//! A small offline calculator for the search field.
//!
//! Recursive-descent over a fixed grammar; nothing is ever handed to a
//! shell or interpreter:
//!
//! ```text
//! expr   := term (('+' | '-') term)*
//! term   := unary (('*' | '/' | '%') unary)*
//! unary  := ('-' | '+') unary | power
//! power  := atom ('^' unary)?          (right-associative)
//! atom   := number | constant | func '(' expr ')' | '(' expr ')'
//! ```

const MAX_DEPTH: u32 = 64;
const MAX_INPUT: usize = 256;

/// The value of `input` if it is an arithmetic expression worth showing:
/// at least one operator or function, so that plain numbers ("2026") and
/// words do not turn into calculator results.
pub fn evaluate(input: &str) -> Option<f64> {
    let s = input.trim();
    if s.is_empty() || s.len() > MAX_INPUT {
        return None;
    }
    let tokens = lex(s)?;
    let interesting = tokens.iter().any(|t| matches!(t, Tok::Op(_) | Tok::Ident(_)))
        && !matches!(tokens.as_slice(), [Tok::Op('-' | '+'), Tok::Num(_)]);
    if !interesting {
        return None;
    }
    let mut p = Parser { tokens, pos: 0, depth: 0 };
    let v = p.expr()?;
    (p.pos == p.tokens.len() && v.is_finite()).then_some(v)
}

/// Display form: integers without a fraction, others with up to 10
/// significant decimals and no trailing zeros.
pub fn format(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.10}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_string() } else { s.to_string() }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Op(char),
    LParen,
    RParen,
    Ident(String),
}

fn lex(s: &str) -> Option<Vec<Tok>> {
    let b: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            ' ' | '\t' => i += 1,
            '0'..='9' | '.' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') {
                    i += 1;
                }
                let text: String = b[start..i].iter().collect();
                out.push(Tok::Num(text.parse().ok()?));
            }
            '+' | '-' | '*' | '/' | '%' | '^' => {
                out.push(Tok::Op(c));
                i += 1;
            }
            '×' => {
                out.push(Tok::Op('*'));
                i += 1;
            }
            '÷' => {
                out.push(Tok::Op('/'));
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            'a'..='z' | 'A'..='Z' | 'π' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphabetic() || b[i] == 'π') {
                    i += 1;
                }
                out.push(Tok::Ident(b[start..i].iter().collect::<String>().to_lowercase()));
            }
            _ => return None,
        }
    }
    Some(out)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
    depth: u32,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn enter(&mut self) -> Option<()> {
        self.depth += 1;
        (self.depth <= MAX_DEPTH).then_some(())
    }

    fn expr(&mut self) -> Option<f64> {
        self.enter()?;
        let mut v = self.term()?;
        while let Some(Tok::Op(op @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let r = self.term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        self.depth -= 1;
        Some(v)
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        while let Some(Tok::Op(op @ ('*' | '/' | '%'))) = self.peek().cloned() {
            self.pos += 1;
            let r = self.unary()?;
            v = match op {
                '*' => v * r,
                '/' if r == 0.0 => return None,
                '/' => v / r,
                _ if r == 0.0 => return None,
                _ => v % r,
            };
        }
        Some(v)
    }

    fn unary(&mut self) -> Option<f64> {
        self.enter()?;
        let v = match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                -self.unary()?
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.unary()?
            }
            _ => self.power()?,
        };
        self.depth -= 1;
        Some(v)
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.atom()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.pos += 1;
            let exp = self.unary()?;
            return Some(base.powf(exp));
        }
        Some(base)
    }

    fn atom(&mut self) -> Option<f64> {
        match self.next()? {
            Tok::Num(n) => Some(n),
            Tok::LParen => {
                let v = self.expr()?;
                (self.next()? == Tok::RParen).then_some(v)
            }
            Tok::Ident(name) => match name.as_str() {
                "pi" | "π" => Some(std::f64::consts::PI),
                "e" => Some(std::f64::consts::E),
                f => {
                    if self.next()? != Tok::LParen {
                        return None;
                    }
                    let x = self.expr()?;
                    if self.next()? != Tok::RParen {
                        return None;
                    }
                    apply(f, x)
                }
            },
            _ => None,
        }
    }
}

fn apply(f: &str, x: f64) -> Option<f64> {
    Some(match f {
        "sqrt" if x >= 0.0 => x.sqrt(),
        "abs" => x.abs(),
        "sin" => x.sin(),
        "cos" => x.cos(),
        "tan" => x.tan(),
        "ln" if x > 0.0 => x.ln(),
        "log" if x > 0.0 => x.log10(),
        "round" => x.round(),
        "floor" => x.floor(),
        "ceil" => x.ceil(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> Option<String> {
        evaluate(s).map(format)
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(ev("2+3*4").as_deref(), Some("14"));
        assert_eq!(ev("(2+3)*4").as_deref(), Some("20"));
        assert_eq!(ev("2^3^2").as_deref(), Some("512"), "right-associative");
        assert_eq!(ev("-2^2").as_deref(), Some("-4"));
        assert_eq!(ev("10 % 3").as_deref(), Some("1"));
        assert_eq!(ev("1/3").as_deref(), Some("0.3333333333"));
        assert_eq!(ev("0.1+0.2").as_deref(), Some("0.3"));
        assert_eq!(ev("6 × 7 ÷ 2").as_deref(), Some("21"));
        assert_eq!(ev("sqrt(16) + abs(-2)").as_deref(), Some("6"));
        assert_eq!(ev("2*pi").as_deref(), Some("6.2831853072"));
    }

    #[test]
    fn only_expressions_count() {
        for s in ["", "2026", "-5", "report", "notes 2024", "a+b", "rm -rf /", "1.2.3+1", "foo(2)", "sqrt 4"] {
            assert_eq!(evaluate(s), None, "{s:?}");
        }
    }

    #[test]
    fn errors_are_none_not_panics() {
        for s in ["1/0", "5%0", "sqrt(-1)", "ln(0)", "(1+2", "1+", "*2", "((((", "9^9^9^9", "()"] {
            assert_eq!(evaluate(s), None, "{s:?}");
        }
        let deep = format!("{}1{}", "(".repeat(500), ")".repeat(500));
        assert_eq!(evaluate(&deep), None, "too long");
        let deep = format!("{}1+1{}", "(".repeat(100), ")".repeat(100));
        assert_eq!(evaluate(&deep), None, "too deep");
        assert_eq!(evaluate(&"-".repeat(200).to_string()), None);
    }
}
