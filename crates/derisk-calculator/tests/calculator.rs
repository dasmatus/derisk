use derisk_calculator::{CalculatorApp, Error, evaluate, format_number};
use mcsapi_ui::{Theme, egui, run_frame};

fn eval(text: &str) -> f64 {
    evaluate(text, 0.0).unwrap()
}

#[test]
fn follows_precedence_and_associativity() {
    assert_eq!(eval("1 + 2 * 3"), 7.0);
    assert_eq!(eval("(1 + 2) * 3"), 9.0);
    assert_eq!(eval("2 ^ 3 ^ 2"), 512.0);
    assert_eq!(eval("-2 ^ 2"), -4.0);
    assert_eq!(eval("2 ^ -1"), 0.5);
    assert_eq!(eval("10 - 4 - 3"), 3.0);
    assert_eq!(eval("7 % 4"), 3.0);
    assert_eq!(eval("8 ÷ 2 × 3 − 1"), 11.0);
    assert_eq!(eval("--3"), 3.0);
}

#[test]
fn knows_functions_and_constants() {
    assert_eq!(eval("sqrt(16) + abs(-2)"), 6.0);
    assert!((eval("sin(pi / 2)") - 1.0).abs() < 1e-12);
    assert!((eval("ln(e)") - 1.0).abs() < 1e-12);
    assert_eq!(eval("log(1000)"), 3.0);
    assert_eq!(eval("round(2.5) + floor(1.9) + ceil(1.1)"), 6.0);
    assert_eq!(evaluate("ans * 2", 21.0), Ok(42.0));
    assert_eq!(eval("PI"), std::f64::consts::PI);
}

#[test]
fn reports_errors() {
    assert_eq!(evaluate("1 / 0", 0.0), Err(Error::DivisionByZero));
    assert_eq!(evaluate("5 % 0", 0.0), Err(Error::DivisionByZero));
    assert_eq!(evaluate("1 +", 0.0), Err(Error::UnexpectedEnd));
    assert_eq!(evaluate("(1", 0.0), Err(Error::UnexpectedEnd));
    assert_eq!(
        evaluate("1 2", 0.0),
        Err(Error::UnexpectedToken("2".into()))
    );
    assert_eq!(
        evaluate("foo(1)", 0.0),
        Err(Error::UnknownName("foo".into()))
    );
    assert_eq!(evaluate("2 $ 2", 0.0), Err(Error::UnexpectedChar('$')));
    assert_eq!(
        evaluate("1.2.3", 0.0),
        Err(Error::UnexpectedToken("1.2.3".into()))
    );
    assert_eq!(evaluate("sqrt(-1)", 0.0), Err(Error::NotFinite));
    assert_eq!(evaluate("10 ^ 400", 0.0), Err(Error::NotFinite));
}

#[test]
fn formats_results_compactly() {
    assert_eq!(format_number(42.0), "42");
    assert_eq!(format_number(0.1 + 0.2), "0.3");
    assert_eq!(format_number(-1.5), "-1.5");
    assert_eq!(format_number(1.0 / 3.0), "0.333333333333");
    assert_eq!(format_number(1e20), "1.000000e20");
    assert_eq!(format_number(0.0), "0");
}

#[test]
fn app_keeps_history_and_ans() {
    let mut app = CalculatorApp::default();
    app.input = "6 * 7".into();
    app.submit();
    assert_eq!(app.input, "42");
    assert_eq!(app.ans(), 42.0);
    app.input = "ans / 2".into();
    app.submit();
    assert_eq!(app.history.len(), 2);
    assert_eq!(app.ans(), 21.0);
    app.input = "1 +".into();
    app.submit();
    assert_eq!(app.error(), Some(&Error::UnexpectedEnd));
    assert_eq!(app.history.len(), 2);
    app.input.push('2');
    assert_eq!(app.error(), None, "an edit makes the error stale");
    app.input.pop();
    let mut output = run_frame(
        &mut app,
        &egui::Context::default(),
        egui::RawInput::default(),
        &Theme::default(),
    );
    assert!(!output.shapes.is_empty());
    output.textures_delta.clear();
}
