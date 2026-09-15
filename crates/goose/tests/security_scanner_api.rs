use goose::security::scanner::PromptInjectionScanner;

#[test]
fn public_security_scanner_ml_constructor_is_compatible() {
    fn accepts_constructor(_: fn() -> anyhow::Result<PromptInjectionScanner>) {}

    accepts_constructor(PromptInjectionScanner::with_ml_detection);
}
