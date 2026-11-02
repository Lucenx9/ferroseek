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

/// Whether the query explicitly requests the calculator, including an empty
/// or incomplete expression.
pub fn is_explicit(input: &str) -> bool {
    input.trim_start().starts_with('=')
}

/// Evaluates expressions, or plain numbers when prefixed with `=`. Without
/// that prefix, plain numbers and constants (`e`, `pi`, also signed) stay
/// available for application and file search.
pub fn evaluate(input: &str) -> Option<f64> {
    let s = input.trim();
    if s.is_empty() || s.len() > MAX_INPUT {
        return None;
    }
    let (s, explicit) = match s.strip_prefix('=') {
        Some(expression) => (expression.trim(), true),
        None => (s, false),
    };
    let tokens = lex(s)?;
    let interesting = tokens.iter().any(|t| matches!(t, Tok::Op(_) | Tok::Ident(_)))
        && !matches!(
            tokens.as_slice(),
            [Tok::Num(_) | Tok::Ident(_)] | [Tok::Op('-' | '+'), Tok::Num(_) | Tok::Ident(_)]
        );
    if !explicit && !interesting {
        return None;
    }
    let mut p = Parser { tokens, pos: 0, depth: 0 };
    let v = p.expr()?;
    (p.pos == p.tokens.len() && v.is_finite()).then_some(v)
}

/// Reuses the unrounded result as an editable calculator query. Negative
/// operands need parentheses so an appended power applies to the whole value.
pub fn continuation(input: &str) -> Option<String> {
    let v = evaluate(input)?;
    let value = if v == 0.0 {
        "0".to_string()
    } else if (1e-4..1e15).contains(&v.abs()) {
        v.to_string()
    } else {
        format!("{v:e}")
    };
    Some(if v < 0.0 { format!("= ({value})") } else { format!("= {value}") })
}

/// Significant digits shown (and copied) for non-integer results.
const SIGNIFICANT: i32 = 10;

/// Display form with up to 10 significant digits and no trailing zeros:
/// integers as integers, ordinary magnitudes in fixed point, very small or
/// very large values in scientific notation (so `10^-12` never becomes 0).
pub fn format(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    let a = v.abs();
    if v == v.trunc() && a < 1e15 {
        return format!("{}", v as i64);
    }
    if (1e-4..1e15).contains(&a) {
        let int_digits = a.log10().floor() as i32 + 1;
        let decimals = (SIGNIFICANT - int_digits).max(0) as usize;
        let s = format!("{v:.decimals$}");
        let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { &s };
        return s.to_string();
    }
    let s = format!("{v:.prec$e}", prec = (SIGNIFICANT - 1) as usize);
    let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let mantissa = if mantissa.contains('.') { mantissa.trim_end_matches('0').trim_end_matches('.') } else { mantissa };
    format!("{mantissa}e{exp}")
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
                if matches!(b.get(i), Some('e' | 'E')) {
                    i += 1;
                    if matches!(b.get(i), Some('+' | '-')) {
                        i += 1;
                    }
                    let exponent = i;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i == exponent {
                        return None;
                    }
                }
                let text: String = b[start..i].iter().collect();
                let value: f64 = text.parse().ok()?;
                if !value.is_finite() {
                    return None;
                }
                out.push(Tok::Num(value));
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
        assert_eq!(ev("2*pi").as_deref(), Some("6.283185307"));
    }

    #[test]
    fn small_and_large_results_keep_significant_digits() {
        assert_eq!(ev("10^-12").as_deref(), Some("1e-12"));
        assert_eq!(ev("-1/3000000").as_deref(), Some("-3.333333333e-7"));
        assert_eq!(ev("1/4000").as_deref(), Some("0.00025"));
        assert_eq!(ev("2^60").as_deref(), Some("1.152921505e18"));
        assert_eq!(ev("12345.678901234*1").as_deref(), Some("12345.6789"));
        assert_eq!(ev("0.1*3").as_deref(), Some("0.3"));
        assert_eq!(ev("-0.5*0").as_deref(), Some("0"));
    }

    #[test]
    fn only_expressions_count() {
        for s in [
            "",
            "2026",
            "-5",
            "+5",
            "1e3",
            "-1e-12",
            "report",
            "notes 2024",
            "a+b",
            "rm -rf /",
            "1.2.3+1",
            "foo(2)",
            "sqrt 4",
            "e",
            "pi",
            "PI",
            "π",
            "-e",
            "+pi",
        ] {
            assert_eq!(evaluate(s), None, "{s:?}");
        }
    }

    #[test]
    fn explicit_mode_accepts_numbers_and_keeps_incomplete_input_in_mode() {
        assert_eq!(evaluate("=42"), Some(42.0));
        assert_eq!(evaluate("  = -5  "), Some(-5.0));
        assert_eq!(evaluate("= +5"), Some(5.0));
        assert_eq!(evaluate("= (2+3)*4"), Some(20.0));
        for s in ["=", " = ", "= 1+", "==2", "= report"] {
            assert!(is_explicit(s));
            assert_eq!(evaluate(s), None, "{s:?}");
        }
        for s in ["", "2026", "1+1", "report=1"] {
            assert!(!is_explicit(s));
        }
    }

    #[test]
    fn scientific_numbers_require_a_complete_finite_literal() {
        assert_eq!(evaluate("1e-12*2"), Some(2e-12));
        assert_eq!(evaluate("2E+3+1"), Some(2001.0));
        assert_eq!(evaluate("= -.5E-2"), Some(-0.005));
        assert_eq!(evaluate("e+1"), Some(std::f64::consts::E + 1.0));
        assert_eq!(evaluate("= e"), Some(std::f64::consts::E));
        for s in ["= 1e", "= 1e+", "= 1e-", "= 1e--2", "= 1.2.3e4", "= 1e2.5", "= 1e309", "1e309^0", "1/1e309"] {
            assert_eq!(evaluate(s), None, "{s:?}");
        }
    }

    #[test]
    fn continuation_preserves_precision_and_negative_operand_precedence() {
        let third = continuation("1/3").unwrap();
        assert_eq!(third, "= 0.3333333333333333");
        assert_eq!(evaluate(&format!("{third}*3")), Some(1.0));
        let negative = continuation("-2^2").unwrap();
        assert_eq!(negative, "= (-4)");
        assert_eq!(evaluate(&format!("{negative}^2")), Some(16.0));
        let tiny = continuation("10^-12").unwrap();
        assert_eq!(tiny, "= 1e-12");
        assert_eq!(evaluate(&format!("{tiny}*2")), Some(2e-12));
        let huge = continuation("2^60").unwrap();
        assert_eq!(evaluate(&format!("{huge}/2")), Some(2f64.powi(59)));
        assert_eq!(continuation("= -0").as_deref(), Some("= 0"));
        for s in ["report", "=", "= 1/0"] {
            assert_eq!(continuation(s), None, "{s:?}");
        }
    }

    #[test]
    fn continuation_round_trips_finite_extremes_within_the_input_limit() {
        for s in
            ["= 5e-324", "= -5e-324", "= 1.7976931348623157e308", "= -1.7976931348623157e308", "= 0.0001", "= 1e15"]
        {
            let next = continuation(s).unwrap();
            assert!(next.len() <= MAX_INPUT, "{next}");
            assert_eq!(evaluate(&next), evaluate(s), "{s:?}");
        }
        let max = format!("= {}", "1".repeat(MAX_INPUT - 2));
        assert!(evaluate(&max).is_some());
        assert_eq!(evaluate(&format!("{max}1")), None, "the prefix counts towards the limit");
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
        assert_eq!(evaluate(&format!("= {deep}")), None, "explicit mode keeps the depth limit");
        for s in ["= 1/0", "= 5%0", "= sqrt(-1)", "= 9^9^9^9"] {
            assert_eq!(evaluate(s), None, "{s:?}");
        }
    }
}
