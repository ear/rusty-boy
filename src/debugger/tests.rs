use super::dbglanguage;
use super::language::*;
use super::dbglanguage::*;

#[cfg(test)]
#[test]
fn number_test() {
    assert!(parse_Number("12").is_ok());
    assert!(parse_Number("-02").is_ok());
}

#[test]
fn hexnumber_test() {
    assert!(parse_Number("0x12").is_ok());
    assert_eq!(parse_Input("0x100").unwrap(),
               DebuggerAction::Echo { str: "0x100".to_string() });
}