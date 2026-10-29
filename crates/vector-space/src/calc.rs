//! A small calculator: + - * / % ^, parentheses, functions, constants,
//! hex/binary input. Used by `=` and inline in the app search.

pub fn eval(src: &str) -> Option<f64> {
    let toks = lex(src)?;
    let mut p = Parser { t: &toks, i: 0 };
    let v = p.expr()?;
    (p.i == toks.len() && v.is_finite()).then_some(v)
}

/// Looks like arithmetic (has an operator or a function), not a plain word or number.
pub fn looks_like_math(src: &str) -> bool {
    let s = src.trim();
    !s.is_empty()
        && s.chars().any(|c| c.is_ascii_digit())
        && (s.chars().any(|c| "+*/^%()".contains(c)) || s[1..].contains('-'))
        && eval(s).is_some()
}

/// Rounds away float noise: 0.1+0.2 shows 0.3.
pub fn format(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{:.12}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "0" || s == "-0" { format!("{v:e}") } else { s }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Id(String),
    Op(char),
}

fn lex(s: &str) -> Option<Vec<Tok>> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() || ch == '_' && i > 0 && c[i - 1].is_ascii_digit() {
            i += 1;
        } else if ch == '0' && i + 1 < c.len() && (c[i + 1] == 'x' || c[i + 1] == 'b') {
            let radix = if c[i + 1] == 'x' { 16 } else { 2 };
            let mut j = i + 2;
            while j < c.len() && c[j].is_digit(radix) {
                j += 1;
            }
            let t: String = c[i + 2..j].iter().collect();
            out.push(Tok::Num(i64::from_str_radix(&t, radix).ok()? as f64));
            i = j;
        } else if ch.is_ascii_digit() || ch == '.' {
            let mut j = i;
            while j < c.len() && (c[j].is_ascii_digit() || c[j] == '.' || c[j] == '_') {
                j += 1;
            }
            if j < c.len() && (c[j] == 'e' || c[j] == 'E') && j + 1 < c.len() && (c[j + 1].is_ascii_digit() || (c[j + 1] == '-' || c[j + 1] == '+') && j + 2 < c.len() && c[j + 2].is_ascii_digit()) {
                j += 2;
                while j < c.len() && c[j].is_ascii_digit() {
                    j += 1;
                }
            }
            let t: String = c[i..j].iter().filter(|c| **c != '_').collect();
            out.push(Tok::Num(t.parse().ok()?));
            i = j;
        } else if ch.is_alphabetic() {
            let mut j = i;
            while j < c.len() && c[j].is_alphanumeric() {
                j += 1;
            }
            out.push(Tok::Id(c[i..j].iter().collect::<String>().to_lowercase()));
            i = j;
        } else if "+-*/%^()×÷−".contains(ch) {
            out.push(Tok::Op(match ch {
                '×' => '*',
                '÷' => '/',
                '−' => '-',
                o => o,
            }));
            i += 1;
        } else {
            return None;
        }
    }
    Some(out)
}

struct Parser<'a> {
    t: &'a [Tok],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn eat(&mut self, op: char) -> bool {
        if self.peek() == Some(&Tok::Op(op)) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            if self.eat('+') {
                v += self.term()?;
            } else if self.eat('-') {
                v -= self.term()?;
            } else {
                return Some(v);
            }
        }
    }
    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            if self.eat('*') {
                v *= self.unary()?;
            } else if self.eat('/') {
                v /= self.unary()?;
            } else if self.eat('%') {
                v %= self.unary()?;
            } else if matches!(self.peek(), Some(Tok::Op('(')) | Some(Tok::Id(_))) {
                // implicit multiplication: 2pi, 3(4+1)
                v *= self.unary()?;
            } else {
                return Some(v);
            }
        }
    }
    fn unary(&mut self) -> Option<f64> {
        if self.eat('-') {
            return Some(-self.unary()?);
        }
        if self.eat('+') {
            return self.unary();
        }
        self.power()
    }
    fn power(&mut self) -> Option<f64> {
        let b = self.atom()?;
        if self.eat('^') {
            // right-associative, and binds tighter than a leading minus on its right
            let e = self.unary()?;
            return Some(b.powf(e));
        }
        Some(b)
    }
    fn atom(&mut self) -> Option<f64> {
        match self.peek()?.clone() {
            Tok::Num(n) => {
                self.i += 1;
                Some(n)
            }
            Tok::Op('(') => {
                self.i += 1;
                let v = self.expr()?;
                self.eat(')').then_some(v)
            }
            Tok::Id(name) => {
                self.i += 1;
                match name.as_str() {
                    "pi" | "π" => return Some(std::f64::consts::PI),
                    "e" => return Some(std::f64::consts::E),
                    "tau" => return Some(std::f64::consts::TAU),
                    _ => {}
                }
                let a = if self.peek() == Some(&Tok::Op('(')) { self.atom()? } else { self.power()? };
                Some(match name.as_str() {
                    "sqrt" => a.sqrt(),
                    "cbrt" => a.cbrt(),
                    "abs" => a.abs(),
                    "ln" => a.ln(),
                    "log" => a.log10(),
                    "log2" => a.log2(),
                    "exp" => a.exp(),
                    "sin" => a.sin(),
                    "cos" => a.cos(),
                    "tan" => a.tan(),
                    "asin" => a.asin(),
                    "acos" => a.acos(),
                    "atan" => a.atan(),
                    "round" => a.round(),
                    "floor" => a.floor(),
                    "ceil" => a.ceil(),
                    _ => return None,
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic() {
        assert_eq!(eval("1+2*3"), Some(7.0));
        assert_eq!(eval("(1+2)*3"), Some(9.0));
        assert_eq!(eval("2^3^2"), Some(512.0));
        assert_eq!(eval("-2^2"), Some(-4.0));
        assert_eq!(eval("10 % 4"), Some(2.0));
        assert_eq!(eval("2pi"), Some(std::f64::consts::TAU));
        assert_eq!(eval("sqrt(16)+0x10"), Some(20.0));
        assert_eq!(eval("1e3/4"), Some(250.0));
        assert_eq!(eval("1_000*2"), Some(2000.0));
        assert_eq!(eval("1/0"), None);
        assert_eq!(eval("hello"), None);
        assert_eq!(eval("2+"), None);
    }

    #[test]
    fn detection_and_format() {
        assert!(looks_like_math("2+2"));
        assert!(!looks_like_math("firefox"));
        assert!(!looks_like_math("42"));
        assert!(!looks_like_math("-5"));
        assert_eq!(format(0.1 + 0.2), "0.3");
        assert_eq!(format(4.0), "4");
    }
}
