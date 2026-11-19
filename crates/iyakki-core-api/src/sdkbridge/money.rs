//! Spending limits in a currency (`iyakki.money`). Pure rules; the host keeps the numbers and asks the person.
//!
//! * Amounts are whole **minor units** of a currency (cents, pence, yen for a currency without a fraction). Never floating point.
//! * An app has one currency (the one it declared) and a limit per day. The default limit is **zero**: nothing may be spent until the
//!   person raises it. Only the person raises it (the Info panel or a chat tool that asks); the app can ask, and the answer is a card.
//! * The limit is checked **before every spend**, against what was already spent today. No provider is called by this module.

/// Largest amount (or limit) accepted, in minor units: far above any real purchase and far below overflow.
pub const MAX_MINOR: u64 = 1_000_000_000_000;

/// Currencies without a minor unit (zero decimals) and with three decimals; everything else has two.
const ZERO: [&str; 8] = ["JPY", "KRW", "VND", "CLP", "ISK", "UGX", "XAF", "XOF"];
const THREE: [&str; 5] = ["BHD", "KWD", "JOD", "OMR", "TND"];

pub fn valid_currency(s: &str) -> bool {
    s.len() == 3 && s.chars().all(|c| c.is_ascii_uppercase())
}

/// Digits after the decimal point.
pub fn exponent(currency: &str) -> u32 {
    if ZERO.contains(&currency) {
        0
    } else if THREE.contains(&currency) {
        3
    } else {
        2
    }
}

/// `3.50 USD`, `500 JPY`.
pub fn format_minor(amount: u64, currency: &str) -> String {
    let e = exponent(currency);
    if e == 0 {
        return format!("{amount} {currency}");
    }
    let unit = 10u64.pow(e);
    format!("{}.{:0width$} {currency}", amount / unit, amount % unit, width = e as usize)
}

/// A limit the person set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limit {
    pub currency: String,
    pub per_day_minor: u64,
}

/// Why a spend was refused. The wording the app and the person see comes from [`Refusal::message`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The default: the person has not set a limit above nothing.
    NoLimit,
    /// The amount is zero or absurd.
    BadAmount,
    /// Another currency than the app's.
    OtherCurrency { allowed: String },
    /// More than what is left today.
    Over { remaining: u64, currency: String },
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::NoLimit => "this app may not spend money: its daily limit is nothing until you raise it".into(),
            Refusal::BadAmount => "amount_minor is a whole number of the currency's smallest unit, more than 0".into(),
            Refusal::OtherCurrency { allowed } => format!("this app may spend only in {allowed}"),
            Refusal::Over { remaining, currency } => format!("that is more than the {} this app may still spend today", format_minor(*remaining, currency)),
        }
    }
}

/// Decide a spend of `amount` `currency` given the limit and what was already spent today. `Ok` is the amount left after it.
pub fn check_spend(limit: Option<&Limit>, spent_today: u64, currency: &str, amount: u64) -> Result<u64, Refusal> {
    if amount == 0 || amount > MAX_MINOR || !valid_currency(currency) {
        return Err(Refusal::BadAmount);
    }
    let Some(l) = limit.filter(|l| l.per_day_minor > 0) else { return Err(Refusal::NoLimit) };
    if l.currency != currency {
        return Err(Refusal::OtherCurrency { allowed: l.currency.clone() });
    }
    let remaining = l.per_day_minor.saturating_sub(spent_today);
    if amount > remaining {
        return Err(Refusal::Over { remaining, currency: currency.into() });
    }
    Ok(remaining - amount)
}
