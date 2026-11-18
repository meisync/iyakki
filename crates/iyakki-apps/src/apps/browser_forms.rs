//! Form helpers beyond passwords (F-BROW-96): a person's name, e-mail, phone and address kept in their personal vault (never in a
//! plain file) and filled into the fields of a form that clearly ask for them. A field is recognised from its name and placeholder
//! only; anything that looks like a card, an account, a secret or an identity document is never touched, and a field that already
//! holds text is left alone.

use iyakki_web::doc::{Control, ControlKind, Doc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Field {
    Name,
    First,
    Last,
    Email,
    Phone,
    Street,
    City,
    Region,
    Postcode,
    Country,
    Org,
}

pub const ALL: [Field; 11] = [Field::Name, Field::First, Field::Last, Field::Email, Field::Phone, Field::Street, Field::City, Field::Region, Field::Postcode, Field::Country, Field::Org];

impl Field {
    /// The word used to set it: `profile set email me@example.org`.
    pub fn key(self) -> &'static str {
        match self {
            Field::Name => "name",
            Field::First => "first",
            Field::Last => "last",
            Field::Email => "email",
            Field::Phone => "phone",
            Field::Street => "street",
            Field::City => "city",
            Field::Region => "region",
            Field::Postcode => "postcode",
            Field::Country => "country",
            Field::Org => "company",
        }
    }

    pub fn from_key(k: &str) -> Option<Field> {
        let k = k.to_ascii_lowercase();
        ALL.into_iter().find(|f| f.key() == k)
    }
}

/// What a person keeps for filling forms.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub first: String,
    #[serde(default)]
    pub last: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub street: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub postcode: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub company: String,
}

/// Longest value kept for one field.
pub const MAX_VALUE: usize = 200;

impl Profile {
    fn own(&self, f: Field) -> &String {
        match f {
            Field::Name => &self.name,
            Field::First => &self.first,
            Field::Last => &self.last,
            Field::Email => &self.email,
            Field::Phone => &self.phone,
            Field::Street => &self.street,
            Field::City => &self.city,
            Field::Region => &self.region,
            Field::Postcode => &self.postcode,
            Field::Country => &self.country,
            Field::Org => &self.company,
        }
    }

    pub fn get(&self, f: Field) -> String {
        let own = self.own(f);
        if !own.is_empty() {
            return own.clone();
        }
        // a full name, or its two halves, can be made from the other
        match f {
            Field::Name if !self.first.is_empty() || !self.last.is_empty() => format!("{} {}", self.first, self.last).trim().to_string(),
            Field::First => self.name.split_whitespace().next().unwrap_or("").to_string(),
            Field::Last => self.name.split_whitespace().skip(1).collect::<Vec<_>>().join(" "),
            _ => String::new(),
        }
    }

    pub fn set(&mut self, f: Field, value: &str) -> Result<(), String> {
        let v: String = value.trim().chars().filter(|c| !c.is_control()).collect();
        if v.chars().count() > MAX_VALUE {
            return Err(format!("at most {MAX_VALUE} characters"));
        }
        let slot = match f {
            Field::Name => &mut self.name,
            Field::First => &mut self.first,
            Field::Last => &mut self.last,
            Field::Email => &mut self.email,
            Field::Phone => &mut self.phone,
            Field::Street => &mut self.street,
            Field::City => &mut self.city,
            Field::Region => &mut self.region,
            Field::Postcode => &mut self.postcode,
            Field::Country => &mut self.country,
            Field::Org => &mut self.company,
        };
        *slot = v;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        ALL.iter().all(|f| self.own(*f).is_empty())
    }

    /// The fields that hold something, as `key: value` lines.
    pub fn lines(&self) -> Vec<String> {
        ALL.iter().filter(|f| !self.own(**f).is_empty()).map(|f| format!("{}: {}", f.key(), self.own(*f))).collect()
    }
}

const VAULT_NAME: &str = "webform.profile";

/// The profile from the person's vault (empty when none is kept).
pub fn load(cx: &super::AppCx) -> Result<Profile, String> {
    let names = cx.vault().user_names(&cx.session.user).map_err(|_| "Your personal vault is locked; sign in again to use saved details.".to_string())?;
    if !names.iter().any(|n| n == VAULT_NAME) {
        return Ok(Profile::default());
    }
    let text = cx.vault().user_get(&cx.session.user, VAULT_NAME).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|_| "The saved details could not be read.".to_string())
}

pub fn save(cx: &super::AppCx, p: &Profile) -> Result<(), String> {
    if p.is_empty() {
        let _ = cx.vault().user_delete(&cx.session.user, VAULT_NAME); // nothing kept is fine too
        return Ok(());
    }
    let text = serde_json::to_string(p).map_err(|e| e.to_string())?;
    cx.vault().user_set(&cx.session.user, VAULT_NAME, &text).map_err(|e| e.to_string())
}

/// Words of a field's name or placeholder: lower case, split at anything that is not a letter or digit, and at a capital
/// letter that follows a small one (`postCode` is `post`, `code`).
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Words that mean the field is not ours to fill.
const NEVER: &[&str] = &["card", "cc", "cvv", "cvc", "cvn", "iban", "swift", "bic", "ssn", "passport", "birth", "birthday", "dob", "password", "passwd", "pass", "pwd", "pin", "otp", "token", "captcha", "security", "account", "routing", "tax", "licence", "license", "expiry", "expiration", "secret", "answer", "username", "login", "nickname", "handle", "search", "query", "q", "coupon", "promo", "voucher", "gift", "vat", "nif", "tin"];

/// What a text field is asking for, from its name and placeholder.
pub fn classify(c: &Control) -> Option<Field> {
    let mut t = tokens(&c.name);
    t.extend(tokens(&c.placeholder));
    if t.is_empty() || t.iter().any(|w| NEVER.contains(&w.as_str())) {
        return None;
    }
    let has = |ws: &[&str]| t.iter().any(|w| ws.contains(&w.as_str()));
    let joined: String = t.concat();
    if has(&["email", "emailaddress"]) || joined.contains("email") || (has(&["mail"]) && !has(&["mailing"])) {
        return Some(Field::Email);
    }
    if has(&["phone", "tel", "telephone", "mobile", "cell", "cellphone", "msisdn"]) || joined.contains("phone") {
        return Some(Field::Phone);
    }
    if has(&["zip", "zipcode", "postcode", "postal", "pincode", "postalcode"]) || joined.contains("zipcode") || joined.contains("postcode") || joined.contains("postalcode") {
        return Some(Field::Postcode);
    }
    if has(&["city", "town", "locality", "suburb"]) {
        return Some(Field::City);
    }
    if has(&["state", "province", "region", "county", "prefecture"]) {
        return Some(Field::Region);
    }
    if has(&["country", "nation"]) {
        return Some(Field::Country);
    }
    if has(&["company", "organization", "organisation", "org", "business", "employer"]) {
        return Some(Field::Org);
    }
    // the second line of an address (flat, apartment) is not guessed
    if has(&["address2", "addr2", "line2", "apt", "apartment", "suite", "unit", "flat"]) || (has(&["address", "line", "addr"]) && has(&["2", "two"])) {
        return None;
    }
    if has(&["street", "address", "addr", "address1", "addr1", "streetaddress", "mailing"]) || joined.contains("streetaddress") {
        return Some(Field::Street);
    }
    if has(&["first", "fname", "given", "forename", "firstname", "givenname"]) || joined.contains("firstname") || joined.contains("givenname") {
        return Some(Field::First);
    }
    if has(&["last", "lname", "surname", "family", "lastname", "familyname"]) || joined.contains("lastname") || joined.contains("familyname") {
        return Some(Field::Last);
    }
    const NAME_OK: &[&str] = &["name", "fullname", "full", "your", "my", "contact", "customer", "recipient", "billing", "shipping", "ship", "bill", "to", "yourname", "realname", "real"];
    if has(&["name", "fullname", "yourname", "realname"]) && t.iter().all(|w| NAME_OK.contains(&w.as_str())) {
        return Some(Field::Name);
    }
    None
}

/// One field to fill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    pub control: usize,
    pub field: Field,
    /// The text for a text field.
    pub value: String,
    /// For a drop-down: the option to choose.
    pub option: Option<usize>,
}

fn option_for(c: &Control, want: &str) -> Option<usize> {
    let w = want.trim().to_lowercase();
    if w.is_empty() {
        return None;
    }
    c.options.iter().position(|o| o.value.to_lowercase() == w || o.label.trim().to_lowercase() == w).or_else(|| c.options.iter().position(|o| o.label.trim().to_lowercase().starts_with(&w) && w.len() >= 3))
}

/// What to fill in the form that `scope` names, or, when `scope` is `None`, in the form with the most recognisable fields.
/// `current(i)` is what control `i` holds now (a field that holds text is left alone). Each kind of field is filled once per form.
pub fn plan(doc: &Doc, profile: &Profile, scope: Option<Option<usize>>, current: &dyn Fn(usize) -> String) -> Vec<Fill> {
    let mut forms: Vec<Option<usize>> = doc.controls.iter().map(|c| c.form).collect();
    forms.sort();
    forms.dedup();
    let mut best: Vec<Fill> = Vec::new();
    for form in forms {
        if scope.is_some_and(|s| s != form) {
            continue;
        }
        let mut fills: Vec<Fill> = Vec::new();
        for (i, c) in doc.controls.iter().enumerate().filter(|(_, c)| c.form == form) {
            if c.disabled || c.readonly || !matches!(c.kind, ControlKind::Text | ControlKind::Select) {
                continue;
            }
            let Some(f) = classify(c) else { continue };
            if fills.iter().any(|x| x.field == f) {
                continue;
            }
            let value = profile.get(f);
            if value.is_empty() {
                continue;
            }
            match c.kind {
                ControlKind::Select => {
                    if !matches!(f, Field::Country | Field::Region) {
                        continue;
                    }
                    if let Some(o) = option_for(c, &value) {
                        fills.push(Fill { control: i, field: f, value, option: Some(o) });
                    }
                }
                _ => {
                    if !current(i).trim().is_empty() {
                        continue;
                    }
                    fills.push(Fill { control: i, field: f, value, option: None });
                }
            }
        }
        if fills.len() > best.len() {
            best = fills;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_web::doc::from_html;

    fn p() -> Profile {
        Profile { name: "Ada Lovelace".into(), email: "ada@example.org".into(), phone: "+44 20 7946 0000".into(), street: "12 Analytical Way".into(), city: "London".into(), postcode: "N1 9GU".into(), country: "United Kingdom".into(), ..Profile::default() }
    }

    fn kinds(html: &str) -> Vec<Option<Field>> {
        from_html(html).controls.iter().filter(|c| c.kind == ControlKind::Text).map(classify).collect()
    }

    #[test]
    fn fields_are_recognised_from_their_names_and_placeholders_and_secrets_never_are() {
        let k = kinds("<form><input name=email><input name=user_email><input name=phoneNumber><input name=postCode><input name=zip><input name=first_name><input name=lname><input name=fullName><input name=city><input name=addr1><input name=company><input placeholder='Your name'></form>");
        use Field::*;
        assert_eq!(k, vec![Some(Email), Some(Email), Some(Phone), Some(Postcode), Some(Postcode), Some(First), Some(Last), Some(Name), Some(City), Some(Street), Some(Org), Some(Name)]);
        let never = kinds("<form><input name=cc_number><input name=card_name><input name=cvv><input name=ssn><input name=username><input name=search_q><input name=account_name><input name=address2><input name=apartment><input name=hotel><input name=file_name><input name=company_tax_id><input name=q></form>");
        assert!(never.iter().all(Option::is_none), "{never:?}");
        // a user name or a product name is not a person's name
        assert_eq!(kinds("<form><input name=name_of_product><input name=display_name><input name=name></form>"), vec![None, None, Some(Name)]);
    }

    #[test]
    fn the_form_with_the_most_matching_fields_is_filled_and_filled_fields_are_left_alone() {
        let doc = from_html("<form action=/news><input name=email></form><form action=/order><input name=fullname><input name=email><input name=phone><input name=street><input name=city><input name=zip><select name=country><option value=''>Choose<option value=DE>Germany<option value=GB>United Kingdom</select><input type=password name=pw><input name=coupon></form>");
        let plan1 = plan(&doc, &p(), None, &|_| String::new());
        let names: Vec<Field> = plan1.iter().map(|f| f.field).collect();
        assert_eq!(names, vec![Field::Name, Field::Email, Field::Phone, Field::Street, Field::City, Field::Postcode, Field::Country]);
        let country = plan1.last().unwrap();
        assert_eq!((doc.controls[country.control].options[country.option.unwrap()].value.as_str(), country.value.as_str()), ("GB", "United Kingdom"));
        assert!(plan1.iter().all(|f| doc.controls[f.control].form == Some(1)), "the order form, not the newsletter box");
        // something already typed in the e-mail field is not overwritten
        let email_ctl = doc.controls.iter().position(|c| c.name == "email" && c.form == Some(1)).unwrap();
        let plan2 = plan(&doc, &p(), None, &|i| if i == email_ctl { "me@else.org".into() } else { String::new() });
        assert!(!plan2.iter().any(|f| f.field == Field::Email));
        // a named form is the only one looked at
        let only0 = plan(&doc, &p(), Some(Some(0)), &|_| String::new());
        assert_eq!(only0.len(), 1);
        // nothing known, nothing filled
        assert!(plan(&doc, &Profile::default(), None, &|_| String::new()).is_empty());
    }

    #[test]
    fn first_and_last_names_come_from_the_full_name_and_back() {
        let q = p();
        assert_eq!((q.get(Field::First).as_str(), q.get(Field::Last).as_str()), ("Ada", "Lovelace"));
        let r = Profile { first: "Grace".into(), last: "Brewster Murray Hopper".into(), ..Profile::default() };
        assert_eq!(r.get(Field::Name), "Grace Brewster Murray Hopper");
        let mut s = Profile::default();
        assert!(s.is_empty());
        s.set(Field::Email, "  a@b.org\n").unwrap();
        assert_eq!(s.lines(), vec!["email: a@b.org"]);
        assert!(s.set(Field::Street, &"x".repeat(MAX_VALUE + 1)).is_err());
        assert_eq!(Field::from_key("Company"), Some(Field::Org));
        assert_eq!(Field::from_key("ssn"), None, "there is no field for what must not be kept");
    }

    #[test]
    fn the_details_live_in_the_vault_and_in_no_plain_file() {
        use crate::apps::tests::cx_with_admin_store;
        let cx = cx_with_admin_store("forms-profile");
        assert!(load(&cx).unwrap_err().contains("locked"));
        cx.services.on_login("root", "rootpass12", true);
        assert_eq!(load(&cx).unwrap(), Profile::default());
        save(&cx, &p()).unwrap();
        assert_eq!(load(&cx).unwrap(), p());
        assert!(!std::fs::read_dir(cx.data_dir()).unwrap().flatten().any(|e| std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("ada@example.org"))), "in no readable file");
        save(&cx, &Profile::default()).unwrap();
        assert_eq!(load(&cx).unwrap(), Profile::default(), "an empty profile removes it");
    }
}
