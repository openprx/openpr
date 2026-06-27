use rust_decimal::{Decimal, RoundingStrategy};
use std::str::FromStr;

use super::schema::AmountFieldConfig;

pub fn normalize_amount_decimal(raw: &str, config: &AmountFieldConfig) -> Result<String, String> {
    let decimal = Decimal::from_str(raw.trim()).map_err(|_| "amount decimal is invalid".to_string())?;
    if !config.allow_negative && decimal.is_sign_negative() {
        return Err("amount cannot be negative".to_string());
    }
    let rounded = decimal.round_dp_with_strategy(config.scale, rounding_strategy(&config.rounding));
    Ok(format!("{:.*}", config.scale as usize, rounded))
}

fn rounding_strategy(rounding: &str) -> RoundingStrategy {
    match rounding {
        "half_even" => RoundingStrategy::MidpointNearestEven,
        "down" => RoundingStrategy::ToZero,
        "up" => RoundingStrategy::AwayFromZero,
        _ => RoundingStrategy::MidpointAwayFromZero,
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_amount_decimal;
    use crate::forms::schema::AmountFieldConfig;

    fn config() -> AmountFieldConfig {
        AmountFieldConfig {
            currency: "CNY".to_string(),
            scale: 2,
            allow_negative: false,
            rounding: "half_up".to_string(),
        }
    }

    #[test]
    fn decimal_addition_has_no_float_error() {
        let left = "0.1".parse::<rust_decimal::Decimal>().unwrap();
        let right = "0.2".parse::<rust_decimal::Decimal>().unwrap();
        assert_eq!(format!("{:.2}", (left + right).round_dp(2)), "0.30");
    }

    #[test]
    fn normalizes_amount_with_scale() {
        assert_eq!(normalize_amount_decimal("125000", &config()).unwrap(), "125000.00");
        assert_eq!(normalize_amount_decimal("1.005", &config()).unwrap(), "1.01");
    }

    #[test]
    fn rejects_negative_amount_by_default() {
        assert!(normalize_amount_decimal("-1.00", &config()).is_err());
    }
}
