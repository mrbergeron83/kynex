//! What the models cost, per million tokens, for the pickers. The CLI's model
//! list carries no prices, so these are Kynex's own, for the 5.5 models only:
//! Haiku and Opus were checked against what the CLI bills (its `costUSD` on
//! real calls matched them to the cent), Sonnet's was measured the same way.
//! Prices change, and older models differ, so a model not listed shows none
//! rather than a guess. Haiku's rate is for prompts up to 100K tokens; past
//! that it costs more.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    /// USD per million input tokens.
    pub input: f64,
    /// USD per million output tokens.
    pub output: f64,
}

/// The price of a model by its display name ("Haiku 5.5", "opus 5.5"), if known.
pub fn of_name(name: &str) -> Option<Price> {
    let name = name.to_lowercase();
    let price = |input, output| Some(Price { input, output });
    if name.contains("haiku 5.5") {
        price(0.10, 0.50)
    } else if name.contains("sonnet 5.5") {
        price(2.0, 10.0)
    } else if name.contains("opus 5.5") {
        price(4.0, 20.0)
    } else {
        None
    }
}

fn money(x: f64) -> String {
    if x.fract() == 0.0 { format!("${x:.0}") } else { format!("${x:.2}") }
}

/// "$0.10 in · $0.50 out per 1M tokens".
pub fn label(p: Price) -> String {
    format!("{} in · {} out per 1M tokens", money(p.input), money(p.output))
}

/// `description` with the model's price after it, when it has one.
pub fn with_price(name: &str, description: &str) -> String {
    match of_name(name) {
        Some(p) if description.is_empty() => label(p),
        Some(p) => format!("{description} · {}", label(p)),
        None => description.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_the_three_models_and_nothing_else() {
        assert_eq!(label(of_name("Haiku 5.5").unwrap()), "$0.10 in · $0.50 out per 1M tokens");
        assert_eq!(label(of_name("Sonnet 5.5").unwrap()), "$2 in · $10 out per 1M tokens");
        assert_eq!(label(of_name("opus 5.5").unwrap()), "$4 in · $20 out per 1M tokens");
        // Older and other models differ: no guess.
        for n in ["Opus 4.8", "Haiku 4.5", "Fable 5.1", "Default (recommended)", ""] {
            assert!(of_name(n).is_none(), "{n}");
        }
    }

    #[test]
    fn adds_the_price_to_a_description() {
        assert_eq!(
            with_price("Haiku 5.5", "Fastest for quick answers"),
            "Fastest for quick answers · $0.10 in · $0.50 out per 1M tokens"
        );
        assert_eq!(with_price("Haiku 5.5", ""), "$0.10 in · $0.50 out per 1M tokens");
        assert_eq!(with_price("Opus 4.6", "Best"), "Best");
    }
}
