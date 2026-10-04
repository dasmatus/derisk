//! derisk Calculator: a keypad and an expression line with history.
//!
//! [`evaluate`] supports `+ - * / % ^`, parentheses, unary minus, the
//! constants `pi` and `e`, `ans` (the previous result), and the functions
//! `sqrt abs ln log sin cos tan asin acos atan floor ceil round`. Angles are
//! in radians.
//!
//! ```
//! assert_eq!(derisk_calculator::evaluate("2 + 3 * 4 ^ 2", 0.0), Ok(50.0));
//! assert_eq!(derisk_calculator::evaluate("-(ans - 1) / 2", 7.0), Ok(-3.0));
//! assert!(derisk_calculator::evaluate("1 / 0", 0.0).is_err());
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::fmt;

use mcsapi_ui::{App, Theme, egui};

/// Why an expression could not be evaluated.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// A character that is not part of any token.
    UnexpectedChar(char),
    /// The expression ended too early.
    UnexpectedEnd,
    /// A token in the wrong place.
    UnexpectedToken(String),
    /// A name that is neither a constant nor a function.
    UnknownName(String),
    /// Division or remainder by zero.
    DivisionByZero,
    /// The result is not a finite number.
    NotFinite,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedChar(c) => write!(f, "unexpected `{c}`"),
            Self::UnexpectedEnd => f.write_str("incomplete expression"),
            Self::UnexpectedToken(t) => write!(f, "unexpected `{t}`"),
            Self::UnknownName(n) => write!(f, "unknown name `{n}`"),
            Self::DivisionByZero => f.write_str("division by zero"),
            Self::NotFinite => f.write_str("result is not a finite number"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Name(String),
    Op(char),
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => n.fmt(f),
            Self::Name(n) => f.write_str(n),
            Self::Op(c) => write!(f, "{c}"),
        }
    }
}

fn tokenize(text: &str) -> Result<Vec<Token>, Error> {
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c.is_ascii_digit() || c == '.' {
            let mut end = start;
            while let Some(&(i, c)) = chars.peek() {
                if c.is_ascii_digit() || c == '.' {
                    end = i + c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            let number = text[start..end]
                .parse()
                .map_err(|_| Error::UnexpectedToken(text[start..end].into()))?;
            tokens.push(Token::Number(number));
        } else if c.is_alphabetic() {
            let mut end = start;
            while let Some(&(i, c)) = chars.peek() {
                if c.is_alphanumeric() {
                    end = i + c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            tokens.push(Token::Name(text[start..end].to_lowercase()));
        } else {
            let op = match c {
                '×' => '*',
                '÷' => '/',
                '−' => '-',
                c if "+-*/%^()".contains(c) => c,
                c => return Err(Error::UnexpectedChar(c)),
            };
            tokens.push(Token::Op(op));
            chars.next();
        }
    }
    Ok(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    ans: f64,
}

impl Parser<'_> {
    fn peek_op(&self) -> Option<char> {
        match self.tokens.get(self.pos) {
            Some(Token::Op(c)) => Some(*c),
            _ => None,
        }
    }

    fn next(&mut self) -> Result<&Token, Error> {
        let token = self.tokens.get(self.pos).ok_or(Error::UnexpectedEnd)?;
        self.pos += 1;
        Ok(token)
    }

    // sum := product (('+' | '-') product)*
    fn sum(&mut self) -> Result<f64, Error> {
        let mut value = self.product()?;
        while let Some(op @ ('+' | '-')) = self.peek_op() {
            self.pos += 1;
            let rhs = self.product()?;
            value = if op == '+' { value + rhs } else { value - rhs };
        }
        Ok(value)
    }

    // product := unary (('*' | '/' | '%') unary)*
    fn product(&mut self) -> Result<f64, Error> {
        let mut value = self.unary()?;
        while let Some(op @ ('*' | '/' | '%')) = self.peek_op() {
            self.pos += 1;
            let rhs = self.unary()?;
            value = match op {
                '*' => value * rhs,
                _ if rhs == 0.0 => return Err(Error::DivisionByZero),
                '/' => value / rhs,
                _ => value % rhs,
            };
        }
        Ok(value)
    }

    // unary := '-' unary | power
    fn unary(&mut self) -> Result<f64, Error> {
        match self.peek_op() {
            Some('-') => {
                self.pos += 1;
                Ok(-self.unary()?)
            }
            Some('+') => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    // power := atom ('^' unary)?   (right-associative; -2^2 = -4)
    fn power(&mut self) -> Result<f64, Error> {
        let base = self.atom()?;
        if self.peek_op() == Some('^') {
            self.pos += 1;
            return Ok(base.powf(self.unary()?));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<f64, Error> {
        match self.next()?.clone() {
            Token::Number(n) => Ok(n),
            Token::Op('(') => {
                let value = self.sum()?;
                self.expect(')')?;
                Ok(value)
            }
            Token::Name(name) => match name.as_str() {
                "pi" | "π" => Ok(std::f64::consts::PI),
                "e" => Ok(std::f64::consts::E),
                "ans" => Ok(self.ans),
                _ => {
                    let function = function(&name).ok_or(Error::UnknownName(name))?;
                    self.expect('(')?;
                    let argument = self.sum()?;
                    self.expect(')')?;
                    Ok(function(argument))
                }
            },
            token => Err(Error::UnexpectedToken(token.to_string())),
        }
    }

    fn expect(&mut self, op: char) -> Result<(), Error> {
        match self.next()? {
            Token::Op(c) if *c == op => Ok(()),
            token => Err(Error::UnexpectedToken(token.to_string())),
        }
    }
}

fn function(name: &str) -> Option<fn(f64) -> f64> {
    Some(match name {
        "sqrt" => f64::sqrt,
        "abs" => f64::abs,
        "ln" => f64::ln,
        "log" => f64::log10,
        "sin" => f64::sin,
        "cos" => f64::cos,
        "tan" => f64::tan,
        "asin" => f64::asin,
        "acos" => f64::acos,
        "atan" => f64::atan,
        "floor" => f64::floor,
        "ceil" => f64::ceil,
        "round" => f64::round,
        _ => return None,
    })
}

/// Evaluates an expression; `ans` refers to `previous`.
pub fn evaluate(expression: &str, previous: f64) -> Result<f64, Error> {
    let tokens = tokenize(expression)?;
    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
        ans: previous,
    };
    let value = parser.sum()?;
    if let Some(token) = tokens.get(parser.pos) {
        return Err(Error::UnexpectedToken(token.to_string()));
    }
    if value.is_finite() {
        // Avoid showing "-0".
        Ok(if value == 0.0 { 0.0 } else { value })
    } else {
        Err(Error::NotFinite)
    }
}

/// Formats a result with up to 12 significant digits and no trailing zeros.
pub fn format_number(value: f64) -> String {
    if value != 0.0 && !(1e-6..1e15).contains(&value.abs()) {
        return format!("{value:.6e}");
    }
    let integer_digits = if value.abs() < 1.0 {
        0
    } else {
        value.abs().log10().floor() as usize + 1
    };
    let digits = 12 - integer_digits.min(12);
    let text = format!("{value:.digits$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        &text
    };
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// The keypad, row by row, shared by every toolkit's Calculator.
pub const KEYS: [[&str; 5]; 5] = [
    ["C", "(", ")", "%", "⬅"],
    ["7", "8", "9", "÷", "√"],
    ["4", "5", "6", "×", "^"],
    ["1", "2", "3", "−", "pi"],
    ["0", ".", "ans", "+", "="],
];

/// The Calculator app.
#[derive(Debug, Default)]
pub struct CalculatorApp {
    /// The expression being typed.
    pub input: String,
    /// Evaluated expressions and results, oldest first.
    pub history: Vec<(String, f64)>,
    error: Option<Error>,
}

impl CalculatorApp {
    /// The previous result, or zero.
    pub fn ans(&self) -> f64 {
        self.history.last().map_or(0.0, |(_, value)| *value)
    }

    /// The error from the last [`CalculatorApp::submit`], if any.
    pub fn error(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    /// Evaluates the input, records it, and replaces it with the result.
    pub fn submit(&mut self) {
        if self.input.trim().is_empty() {
            return;
        }
        match evaluate(&self.input, self.ans()) {
            Ok(value) => {
                self.history.push((std::mem::take(&mut self.input), value));
                self.input = format_number(value);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// Applies one keypad key from [`KEYS`]: `C` clears, `⬅` deletes,
    /// `=` submits, `√` opens `sqrt(`, anything else is typed.
    pub fn press(&mut self, key: &str) {
        match key {
            "C" => {
                self.input.clear();
                self.error = None;
            }
            "⬅" => {
                self.input.pop();
            }
            "=" => self.submit(),
            "√" => self.input.push_str("sqrt("),
            key => self.input.push_str(key),
        }
    }
}

impl App for CalculatorApp {
    fn title(&self) -> &str {
        "Calculator"
    }

    fn ui(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        egui::Panel::right("calculator-history")
            .default_size(220.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("History");
                    if ui.small_button("Clear").clicked() {
                        self.history.clear();
                    }
                });
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let mut reuse = None;
                        for (expression, value) in &self.history {
                            let text = format!("{expression}\n= {}", format_number(*value));
                            if ui
                                .selectable_label(false, text)
                                .on_hover_text("Use again")
                                .clicked()
                            {
                                reuse = Some(expression.clone());
                            }
                        }
                        if let Some(expression) = reuse {
                            self.input = expression;
                        }
                    });
            });
        egui::CentralPanel::default_margins().show(ui, |ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .font(egui::TextStyle::Heading)
                    .hint_text("2 × (3 + 4)")
                    .desired_width(f32::INFINITY),
            );
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.submit();
                field.request_focus();
            }
            let preview = match &self.error {
                Some(error) => egui::RichText::new(error.to_string()).color(theme.accent),
                None => match evaluate(&self.input, self.ans()) {
                    Ok(value) => egui::RichText::new(format!("= {}", format_number(value))),
                    Err(_) => egui::RichText::new(" "),
                },
            };
            ui.label(preview.size(18.0));
            ui.add_space(8.0);
            let size = egui::vec2(
                ((ui.available_width() - 4.0 * 6.0) / 5.0).max(40.0),
                ((ui.available_height() - 4.0 * 6.0) / 5.0).clamp(32.0, 72.0),
            );
            egui::Grid::new("calculator-keys")
                .spacing([6.0, 6.0])
                .show(ui, |ui| {
                    for row in KEYS {
                        for key in row {
                            let mut label = egui::RichText::new(key).size(18.0);
                            if key == "=" {
                                label = label.color(theme.background);
                            }
                            let mut button = egui::Button::new(label).min_size(size);
                            if key == "=" {
                                button = button.fill(theme.accent);
                            }
                            if ui.add(button).clicked() {
                                self.press(key);
                            }
                        }
                        ui.end_row();
                    }
                });
        });
    }
}
