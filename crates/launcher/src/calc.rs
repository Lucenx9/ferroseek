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

mod units;

const MAX_DEPTH: u32 = 64;
const MAX_INPUT: usize = 256;

/// Whether the query explicitly requests the calculator, including an empty
/// or incomplete expression.
pub fn is_explicit(input: &str) -> bool {
    input.trim_start().starts_with('=')
}

pub struct Answer {
    value: f64,
    unit: Option<&'static str>,
}

impl Answer {
    pub fn label(&self) -> String {
        let value = format(self.value);
        match self.unit {
            Some(unit) => format!("{value} {unit}"),
            None => value,
        }
    }
}

/// Evaluates arithmetic or a conversion in the search field. Plain numbers and
/// constants stay available for search unless prefixed with `=`.
pub fn evaluate(input: &str) -> Option<Answer> {
    let s = input.trim();
    if s.is_empty() || s.len() > MAX_INPUT {
        return None;
    }
    let (s, explicit) = match s.strip_prefix('=') {
        Some(expression) => (expression.trim(), true),
        None => (s, false),
    };
    if let Some((value, unit)) = units::convert(s) {
        return Some(Answer { value, unit: Some(unit) });
    }
    arithmetic(s, explicit).map(|value| Answer { value, unit: None })
}

fn arithmetic(expression: &str, allow_plain: bool) -> Option<f64> {
    let tokens = lex(expression)?;
    let interesting = tokens.iter().any(|t| matches!(t, Tok::Op(_) | Tok::Ident(_) | Tok::LParen))
        && !matches!(
            tokens.as_slice(),
            [Tok::Num(_) | Tok::Ident(_)] | [Tok::Op('-' | '+'), Tok::Num(_) | Tok::Ident(_)]
        );
    if !allow_plain && !interesting {
        return None;
    }
    let mut p = Parser { tokens, pos: 0, depth: 0 };
    let v = p.expr()?;
    (p.pos == p.tokens.len() && v.is_finite()).then_some(v)
}

/// Reuses the unrounded result as an editable calculator query. Negative
/// operands need parentheses so an appended power applies to the whole value.
pub fn continuation(input: &str) -> Option<String> {
    let v = evaluate(input)?.value;
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
fn format(v: f64) -> String {
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
        v.is_finite().then_some(v)
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
        v.is_finite().then_some(v)
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
            let value = base.powf(exp);
            return value.is_finite().then_some(value);
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
        evaluate(s).map(|result| result.label())
    }

    fn value(s: &str) -> Option<f64> {
        evaluate(s).map(|result| result.value)
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(ev("(42)").as_deref(), Some("42"));
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
    fn unit_conversions_accept_arithmetic_amounts_and_display_destination_symbols() {
        for (query, expected) in [
            ("10ft in m", "3.048 m"),
            ("12 in in cm", "30.48 cm"),
            ("= (2 + 3) mi TO km", "8.04672 km"),
            ("sqrt(16) m^2 to ft2", "43.05564167 ft²"),
            ("1e3ms to s", "1 s"),
            ("60 miles per hour to km/h", "96.56064 km/h"),
            ("2LB to kg", "0.90718474 kg"),
            ("1 gallon in L", "3.785411784 L"),
            ("1 imperial gallon in l", "4.54609 L"),
            ("32°F in °C", "0 °C"),
            ("-40 C to F", "-40 °F"),
            ("-459.67 F in K", "0 K"),
            ("1 Kb in b", "1000 b"),
            ("1 kb in Kb", "1 kb"),
            ("1 KB in kB", "1 kB"),
            ("1 MB in Mb", "8 Mb"),
            ("1 MiB in MB", "1.048576 MB"),
        ] {
            assert_eq!(ev(query).as_deref(), Some(expected), "{query}");
        }
    }

    #[test]
    fn temperature_conversion_reuses_absolute_zero_in_each_scale() {
        for (query, expected) in [
            ("-459.67 F to C", "= (-273.15)"),
            ("-273.15 C to F", "= (-459.67)"),
            ("0 K to C", "= (-273.15)"),
            ("0 K to F", "= (-459.67)"),
        ] {
            assert_eq!(continuation(query).as_deref(), Some(expected), "{query}");
        }
    }

    #[test]
    fn conversion_rejects_unknown_units_incompatible_units_and_invalid_values() {
        for query in [
            "reports in m",
            "10 in",
            "10 ft in",
            "1 m in kg",
            "10 °C in m",
            "-273.16 °C in K",
            "-1 K in K",
            "1e308 m in mm",
            "1/0 ft in m",
            "5 m in bananas",
        ] {
            assert!(evaluate(query).is_none(), "{query}");
        }
        let beyond_input_limit = format!("10{}ft in m", " ".repeat(MAX_INPUT));
        assert!(evaluate(&beyond_input_limit).is_none());
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
            assert_eq!(value(s), None, "{s:?}");
        }
    }

    #[test]
    fn explicit_mode_accepts_numbers_and_keeps_incomplete_input_in_mode() {
        assert_eq!(value("=42"), Some(42.0));
        assert_eq!(value("  = -5  "), Some(-5.0));
        assert_eq!(value("= +5"), Some(5.0));
        assert_eq!(value("= (2+3)*4"), Some(20.0));
        for s in ["=", " = ", "= 1+", "==2", "= report"] {
            assert!(is_explicit(s));
            assert_eq!(value(s), None, "{s:?}");
        }
        for s in ["", "2026", "1+1", "report=1"] {
            assert!(!is_explicit(s));
        }
    }

    #[test]
    fn scientific_numbers_require_a_complete_finite_literal() {
        assert_eq!(value("1e-12*2"), Some(2e-12));
        assert_eq!(value("2E+3+1"), Some(2001.0));
        assert_eq!(value("= -.5E-2"), Some(-0.005));
        assert_eq!(value("e+1"), Some(std::f64::consts::E + 1.0));
        assert_eq!(value("= e"), Some(std::f64::consts::E));
        for s in ["= 1e", "= 1e+", "= 1e-", "= 1e--2", "= 1.2.3e4", "= 1e2.5", "= 1e309", "1e309^0", "1/1e309"] {
            assert_eq!(value(s), None, "{s:?}");
        }
    }

    #[test]
    fn continuation_preserves_precision_and_negative_operand_precedence() {
        let third = continuation("1/3").unwrap();
        assert_eq!(third, "= 0.3333333333333333");
        assert_eq!(value(&format!("{third}*3")), Some(1.0));
        let negative = continuation("-2^2").unwrap();
        assert_eq!(negative, "= (-4)");
        assert_eq!(value(&format!("{negative}^2")), Some(16.0));
        let tiny = continuation("10^-12").unwrap();
        assert_eq!(tiny, "= 1e-12");
        assert_eq!(value(&format!("{tiny}*2")), Some(2e-12));
        let huge = continuation("2^60").unwrap();
        assert_eq!(value(&format!("{huge}/2")), Some(2f64.powi(59)));
        let feet = continuation("1 m in ft").unwrap();
        assert_eq!(value(&feet), value("1 m in ft"));
        assert_ne!(value(&feet), Some(3.280839895), "continuation uses the unrounded value");
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
            assert_eq!(value(&next), value(s), "{s:?}");
        }
        let max = format!("= {}", "1".repeat(MAX_INPUT - 2));
        assert!(value(&max).is_some());
        assert_eq!(value(&format!("{max}1")), None, "the prefix counts towards the limit");
    }

    #[test]
    fn invalid_intermediate_results_cannot_become_valid_answers() {
        for s in ["(-1)^0.5", "1^((-1)^0.5)", "1^(-1)^0.5", "1/(10^400)", "1/(1e308*10)", "1/(1e308+1e308)"] {
            assert_eq!(value(s), None, "{s:?}");
            assert_eq!(continuation(s), None, "{s:?}");
            assert_eq!(value(&format!("= {s}")), None, "{s:?}");
        }
        assert_eq!(value("1/(10^300)"), Some(1e-300));
        assert_eq!(value("1^(-1)^2"), Some(1.0));
    }

    #[test]
    fn errors_are_none_not_panics() {
        for s in ["1/0", "5%0", "sqrt(-1)", "ln(0)", "(1+2", "1+", "*2", "((((", "9^9^9^9", "()"] {
            assert_eq!(value(s), None, "{s:?}");
        }
        let deep = format!("{}1{}", "(".repeat(500), ")".repeat(500));
        assert_eq!(value(&deep), None, "too long");
        let deep = format!("{}1+1{}", "(".repeat(100), ")".repeat(100));
        assert_eq!(value(&deep), None, "too deep");
        assert_eq!(value(&"-".repeat(200).to_string()), None);
        assert_eq!(value(&format!("= {deep}")), None, "explicit mode keeps the depth limit");
        for s in ["= 1/0", "= 5%0", "= sqrt(-1)", "= 9^9^9^9"] {
            assert_eq!(value(s), None, "{s:?}");
        }
    }
}
